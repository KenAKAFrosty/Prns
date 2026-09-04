import AccessorySetupKit
import CoreBluetooth
import Foundation
import UIKit

let prnsAccessorySetupStatusDidChange = Notification.Name(
  "rs.reticulum.prns.app.accessory-setup-status-did-change"
)

@MainActor
final class PrnsAccessorySetupCoordinator {
  static let shared = PrnsAccessorySetupCoordinator()

  private enum Phase: String {
    case activating
    case failed
    case ready
    case setupRequired
  }

  private enum PickerPhase: String {
    case idle
    case presented
    case presenting
  }

  private enum NativeStartPhase: String {
    case failed
    case notRequested
    case running
    case starting
    case stopping
  }

  private let session = ASAccessorySession()
  private var activated = false
  private var activationRequested = false
  private var authorizedAccessoryCount = 0
  private var lastError: (code: String, detail: String)?
  private var nativeStartPhase = NativeStartPhase.notRequested
  private var nativeStartCompletions: [(Result<String, Error>) -> Void] = []
  private var nativeStartGeneration: UInt64 = 0
  private var completedStartOutcome: String?
  private var phase = Phase.activating
  private var pickerPhase = PickerPhase.idle
  private var restorationLaunchRequested = false
  private var restorationStartDispatched = false
  private var restorationReady: (() -> Void)?
  private var statusRevision: UInt64 = 0

  private init() {}

  func activate(
    restorationLaunchRequested: Bool,
    restorationReady: @escaping () -> Void
  ) {
    self.restorationLaunchRequested =
      self.restorationLaunchRequested || restorationLaunchRequested
    self.restorationReady = restorationReady
    publish()

    guard !activationRequested else {
      dispatchRestorationStartIfReady()
      return
    }
    activationRequested = true
    session.activate(on: .main) { [weak self] event in
      MainActor.assumeIsolated {
        self?.handle(event)
      }
    }
  }

  func requireAuthorized() throws {
    guard activated, authorizedAccessoryCount > 0, phase == .ready else {
      throw PrnsAppException("Authorize a Bluetooth node before starting prns.")
    }
  }

  func showPicker(_ completion: @escaping (String) -> Void) {
    guard activated else {
      completion(Self.pickerOutcome(type: "notReady"))
      return
    }
    guard UIApplication.shared.applicationState == .active else {
      setLastError(
        code: "pickerRestricted",
        detail: "Bluetooth setup can only open while prns is in the foreground."
      )
      completion(Self.pickerOutcome(type: "restricted"))
      return
    }
    guard pickerPhase == .idle else {
      completion(Self.pickerOutcome(type: "alreadyActive"))
      return
    }

    lastError = nil
    pickerPhase = .presenting
    publish()
    session.showPicker(for: [Self.pickerDisplayItem]) { [weak self] error in
      DispatchQueue.main.async {
        guard let self else {
          completion(Self.pickerOutcome(type: "failed"))
          return
        }
        if let error {
          let outcome = Self.classifyPickerError(error)
          self.setLastError(code: outcome.code, detail: outcome.detail)
          if self.pickerPhase == .presenting {
            self.pickerPhase = .idle
            self.publish()
          }
          completion(Self.pickerOutcome(type: outcome.type))
          return
        }
        if self.pickerPhase == .presenting {
          self.pickerPhase = .idle
          self.publish()
        }
        completion(Self.pickerOutcome(type: "completed"))
      }
    }
  }

  func statusJSON() -> String {
    var object: [String: Any] = [
      "phase": phase.rawValue,
      "picker": pickerPhase.rawValue,
      "authorizedAccessoryCount": authorizedAccessoryCount,
      "nativeStart": nativeStartPhase.rawValue,
      "restorationLaunchRequested": restorationLaunchRequested,
      "revision": statusRevision,
    ]
    if let lastError {
      object["lastError"] = ["code": lastError.code, "detail": lastError.detail]
    } else {
      object["lastError"] = NSNull()
    }
    guard
      let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]),
      let json = String(data: data, encoding: .utf8)
    else {
      return "{\"phase\":\"failed\",\"picker\":\"idle\",\"authorizedAccessoryCount\":0,\"nativeStart\":\"failed\",\"restorationLaunchRequested\":false,\"revision\":0,\"lastError\":{\"code\":\"encodingFailed\",\"detail\":\"Accessory setup status is unavailable.\"}}"
    }
    return json
  }

  func requestNativeStart(
    _ completion: @escaping (Result<String, Error>) -> Void
  ) throws -> UInt64? {
    try requireAuthorized()
    switch nativeStartPhase {
    case .running:
      guard let completedStartOutcome else {
        throw PrnsAppException("The native start result is unavailable.")
      }
      completion(.success(completedStartOutcome))
      return nil
    case .starting:
      guard nativeStartCompletions.count < 8 else {
        throw PrnsAppException("Too many callers are waiting for native startup.")
      }
      nativeStartCompletions.append(completion)
      return nil
    case .stopping:
      throw PrnsAppException(
        "The native node is still stopping. Reset app data before starting it again."
      )
    case .failed, .notRequested:
      nativeStartGeneration &+= 1
      nativeStartPhase = .starting
      nativeStartCompletions = [completion]
      completedStartOutcome = nil
      publish()
      return nativeStartGeneration
    }
  }

  func requireAuthorized(startGeneration: UInt64) throws {
    guard
      nativeStartGeneration == startGeneration,
      nativeStartPhase == .starting
    else {
      throw PrnsAppException("The native start request is no longer current.")
    }
    try requireAuthorized()
  }

  func nativeStartDidFinish(outcomeJSON: String, startGeneration: UInt64) {
    guard
      nativeStartGeneration == startGeneration,
      nativeStartPhase == .starting
    else {
      return
    }
    let cachedOutcome = Self.alreadyRunningOutcome(outcomeJSON)
    nativeStartPhase = cachedOutcome == nil ? .failed : .running
    completedStartOutcome = cachedOutcome
    let completions = nativeStartCompletions
    nativeStartCompletions.removeAll(keepingCapacity: false)
    for completion in completions {
      completion(.success(outcomeJSON))
    }
    publish()
  }

  func nativeStartDidFail(_ error: Error, startGeneration: UInt64) {
    guard
      nativeStartGeneration == startGeneration,
      nativeStartPhase == .starting
    else {
      return
    }
    nativeStartPhase = .failed
    completedStartOutcome = nil
    let completions = nativeStartCompletions
    nativeStartCompletions.removeAll(keepingCapacity: false)
    for completion in completions {
      completion(.failure(error))
    }
    publish()
  }

  func nativeStopWillBegin() {
    nativeStartGeneration &+= 1
    nativeStartPhase = .stopping
    completedStartOutcome = nil
    let completions = nativeStartCompletions
    nativeStartCompletions.removeAll(keepingCapacity: false)
    let error = PrnsAppException("The native start request was superseded by a stop request.")
    for completion in completions {
      completion(.failure(error))
    }
    publish()
  }

  func nativeStopDidFinish(outcomeJSON: String) {
    switch Self.outcomeType(outcomeJSON) {
    case "alreadyStopped", "stopped":
      nativeStartPhase = .notRequested
    default:
      // A failed or malformed stop leaves the Rust supervisor in a potentially
      // stopping state. Do not admit another start until a definitive reset.
      nativeStartPhase = .stopping
    }
    completedStartOutcome = nil
    publish()
  }

  private func handle(_ event: ASAccessoryEvent) {
    switch event.eventType {
    case .activated:
      guard event.error == nil else {
        fail(code: "activationFailed", detail: "Bluetooth accessory setup could not activate.")
        return
      }
      activated = true
      reconcileAuthorizedAccessories()
    case .accessoryAdded, .accessoryChanged, .accessoryRemoved:
      reconcileAuthorizedAccessories()
    case .pickerDidPresent:
      pickerPhase = .presented
      publish()
    case .pickerDidDismiss:
      pickerPhase = .idle
      publish()
    case .invalidated:
      fail(code: "invalidated", detail: "Bluetooth accessory setup became unavailable. Relaunch prns to try again.")
    case .pickerSetupFailed:
      setLastError(code: "connectionFailed", detail: "The selected Bluetooth node could not be set up.")
    default:
      break
    }
  }

  private func reconcileAuthorizedAccessories() {
    authorizedAccessoryCount = session.accessories.filter {
      $0.state == .authorized && $0.bluetoothIdentifier != nil
    }.count
    phase = authorizedAccessoryCount > 0 ? .ready : .setupRequired
    if authorizedAccessoryCount > 0 {
      lastError = nil
      dispatchRestorationStartIfReady()
    }
    publish()
  }

  private func dispatchRestorationStartIfReady() {
    guard
      phase == .ready,
      restorationLaunchRequested,
      !restorationStartDispatched,
      let restorationReady
    else {
      return
    }
    restorationStartDispatched = true
    restorationReady()
  }

  private func fail(code: String, detail: String) {
    phase = .failed
    lastError = (code, detail)
    pickerPhase = .idle
    publish()
  }

  private func setLastError(code: String, detail: String) {
    lastError = (code, detail)
    publish()
  }

  private func publish() {
    statusRevision &+= 1
    let status = statusJSON()
    NotificationCenter.default.post(
      name: prnsAccessorySetupStatusDidChange,
      object: nil,
      userInfo: ["status": status]
    )
    NSLog(
      "PRNS_IOS_ASK phase=%@ picker=%@ authorized=%ld nativeStart=%@ restoration=%@",
      phase.rawValue,
      pickerPhase.rawValue,
      authorizedAccessoryCount,
      nativeStartPhase.rawValue,
      restorationLaunchRequested.description
    )
  }

  private static let pickerDisplayItem: ASPickerDisplayItem = {
    let descriptor = ASDiscoveryDescriptor()
    descriptor.bluetoothServiceUUID = CBUUID(
      string: "37145B00-442D-4A94-917F-8F42C5DA28E3"
    )
    let symbolConfiguration = UIImage.SymbolConfiguration(pointSize: 64, weight: .regular)
    let productImage = UIImage(
      systemName: "antenna.radiowaves.left.and.right",
      withConfiguration: symbolConfiguration
    ) ?? UIImage()
    return ASPickerDisplayItem(
      name: "Reticulum Bluetooth node",
      productImage: productImage,
      descriptor: descriptor
    )
  }()

  private static func classifyPickerError(
    _ error: Error
  ) -> (type: String, code: String, detail: String) {
    let nsError = error as NSError
    guard nsError.domain == ASErrorDomain else {
      return ("failed", "unknown", "Bluetooth setup could not be completed.")
    }
    switch ASError.Code(rawValue: nsError.code) {
    case .userCancelled:
      return ("cancelled", "userCancelled", "Bluetooth setup was cancelled.")
    case .discoveryTimeout:
      return ("timedOut", "discoveryTimeout", "No matching Bluetooth node was found in time.")
    case .pickerRestricted, .userRestricted:
      return ("restricted", "pickerRestricted", "Bluetooth setup is restricted on this device.")
    case .pickerAlreadyActive:
      return ("alreadyActive", "pickerAlreadyActive", "Bluetooth setup is already open.")
    case .invalidated:
      return ("failed", "invalidated", "Bluetooth accessory setup became unavailable.")
    case .activationFailed:
      return ("failed", "activationFailed", "Bluetooth accessory setup could not activate.")
    case .connectionFailed:
      return ("failed", "connectionFailed", "The selected Bluetooth node could not be set up.")
    case .invalidRequest:
      return ("failed", "invalidRequest", "The Bluetooth setup request was invalid.")
    default:
      return ("failed", "unknown", "Bluetooth setup could not be completed.")
    }
  }

  private static func pickerOutcome(type: String) -> String {
    guard
      let data = try? JSONSerialization.data(
        withJSONObject: ["type": type],
        options: [.sortedKeys]
      ),
      let json = String(data: data, encoding: .utf8)
    else {
      return "{\"type\":\"failed\"}"
    }
    return json
  }

  private static func outcomeType(_ json: String) -> String? {
    guard
      let data = json.data(using: .utf8),
      let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
    else {
      return nil
    }
    return object["type"] as? String
  }

  private static func alreadyRunningOutcome(_ json: String) -> String? {
    guard
      let data = json.data(using: .utf8),
      var object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
      let type = object["type"] as? String,
      type == "started" || type == "alreadyRunning",
      object["snapshot"] != nil
    else {
      return nil
    }
    object["type"] = "alreadyRunning"
    guard
      let encoded = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
    else {
      return nil
    }
    return String(data: encoded, encoding: .utf8)
  }
}
