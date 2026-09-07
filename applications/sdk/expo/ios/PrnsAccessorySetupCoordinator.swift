import AccessorySetupKit
import CoreBluetooth
import Foundation
import UIKit

let prnsAccessorySetupStatusDidChange = Notification.Name(
  "rs.reticulum.prns.app.accessory-setup-status-did-change"
)

enum PrnsNativeStartInterruption: LocalizedError {
  case stopInProgress
  case supersededByStop

  var errorDescription: String? {
    switch self {
    case .stopInProgress:
      return "The native node is still stopping. Reset app data before starting it again."
    case .supersededByStop:
      return "The native start request was superseded by a stop request."
    }
  }
}

@MainActor
final class PrnsAccessorySetupCoordinator {
  static let shared = PrnsAccessorySetupCoordinator()

  private typealias Phase = PrnsIosDiagnostics.AccessorySetupPhase
  private typealias PickerPhase = PrnsIosDiagnostics.AccessoryPickerPhase
  private typealias NativeStartPhase = PrnsIosDiagnostics.NativeStartPhase

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
  private var pickerDismissalPending = false
  private var pickerPhase = PickerPhase.idle
  private var restorationDispatch = PrnsRestorationDispatch()
  private var restorationReady: (() -> Void)?
  private var statusRevision: UInt64 = 0

  private init() {}

  func activate(
    restorationLaunchRequested: Bool,
    restorationReady: @escaping () -> Void
  ) {
    restorationDispatch.request(restorationLaunchRequested)
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
    guard nativeStartAuthorized else {
      throw PrnsAppException("Authorize a Bluetooth node before starting prns.")
    }
  }

  func restorationStartAuthorizationDidClose() {
    restorationDispatch.rearmAfterAuthorizationLoss()
    publish()
    dispatchRestorationStartIfReady()
  }

  func showPicker(_ completion: @escaping (String) -> Void) {
    guard activated else {
      completion(Self.pickerOutcome(type: "notReady"))
      return
    }
    guard nativeStartPhase != .starting, nativeStartPhase != .stopping else {
      completion(Self.pickerOutcome(type: "notReady"))
      return
    }
    guard UIApplication.shared.applicationState == .active else {
      setLastError(
        code: "pickerRestricted",
        detail: "The Bluetooth chooser can only open while prns is in the foreground."
      )
      completion(Self.pickerOutcome(type: "restricted"))
      return
    }
    guard pickerPhase == .idle else {
      completion(Self.pickerOutcome(type: "alreadyActive"))
      return
    }

    lastError = nil
    pickerDismissalPending = true
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
            self.pickerDismissalPending = false
            self.pickerPhase = .idle
            self.reconcileAuthorizedAccessories()
          }
          completion(Self.pickerOutcome(type: outcome.type))
          return
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
      "restorationLaunchRequested": restorationDispatch.launchRequested,
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
      throw PrnsNativeStartInterruption.stopInProgress
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
      throw PrnsNativeStartInterruption.supersededByStop
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
    restorationDispatch.cancel()
    nativeStartGeneration &+= 1
    nativeStartPhase = .stopping
    completedStartOutcome = nil
    let completions = nativeStartCompletions
    nativeStartCompletions.removeAll(keepingCapacity: false)
    let error = PrnsNativeStartInterruption.supersededByStop
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
        fail(
          code: "activationFailed",
          detail: "Bluetooth access could not be checked. Relaunch prns to try again."
        )
        return
      }
      activated = true
      reconcileAuthorizedAccessories()
    case .accessoryAdded, .accessoryChanged, .accessoryRemoved:
      reconcileAuthorizedAccessories()
    case .pickerDidPresent:
      pickerDismissalPending = true
      pickerPhase = .presented
      publish()
    case .pickerDidDismiss:
      pickerDismissalPending = false
      pickerPhase = .idle
      reconcileAuthorizedAccessories()
    case .invalidated:
      fail(
        code: "invalidated",
        detail: "Bluetooth access became unavailable. Relaunch prns to try again."
      )
    case .pickerSetupFailed:
      setLastError(
        code: "connectionFailed",
        detail: "The selected Bluetooth node could not be connected."
      )
    default:
      break
    }
  }

  private func reconcileAuthorizedAccessories() {
    let nextAuthorizedAccessoryCount = session.accessories.filter {
      $0.state == .authorized && $0.bluetoothIdentifier != nil
    }.count
    // ASK sends accessoryAdded before pickerDidDismiss. Keep the runtime gated
    // until the system picker is completely out of the way, while still
    // applying removals immediately.
    if pickerDismissalPending && nextAuthorizedAccessoryCount > authorizedAccessoryCount {
      return
    }
    authorizedAccessoryCount = nextAuthorizedAccessoryCount
    phase = authorizedAccessoryCount > 0 ? .ready : .setupRequired
    if authorizedAccessoryCount > 0 {
      lastError = nil
      dispatchRestorationStartIfReady()
    }
    publish()
  }

  private func dispatchRestorationStartIfReady() {
    guard
      let restorationReady,
      restorationDispatch.claimIfAuthorized(nativeStartAuthorized)
    else {
      return
    }
    restorationReady()
  }

  private var nativeStartAuthorized: Bool {
    activated
      && !pickerDismissalPending
      && authorizedAccessoryCount > 0
      && phase == .ready
  }

  private func fail(code: String, detail: String) {
    phase = .failed
    lastError = (code, detail)
    pickerDismissalPending = false
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
    PrnsIosDiagnostics.accessorySetup(
      phase: phase,
      picker: pickerPhase,
      authorizedCount: authorizedAccessoryCount,
      nativeStart: nativeStartPhase,
      restoration: restorationDispatch.launchRequested
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
      return ("failed", "unknown", "Bluetooth access could not be completed.")
    }
    switch ASError.Code(rawValue: nsError.code) {
    case .userCancelled:
      return ("cancelled", "userCancelled", "The Bluetooth chooser was cancelled.")
    case .discoveryTimeout:
      return ("timedOut", "discoveryTimeout", "No matching Bluetooth node was found in time.")
    case .pickerRestricted, .userRestricted:
      return ("restricted", "pickerRestricted", "The Bluetooth chooser is restricted on this device.")
    case .pickerAlreadyActive:
      return ("alreadyActive", "pickerAlreadyActive", "The Bluetooth chooser is already open.")
    case .invalidated:
      return ("failed", "invalidated", "Bluetooth access became unavailable.")
    case .activationFailed:
      return ("failed", "activationFailed", "Bluetooth access could not be checked.")
    case .connectionFailed:
      return ("failed", "connectionFailed", "The selected Bluetooth node could not be connected.")
    case .invalidRequest:
      return ("failed", "invalidRequest", "The Bluetooth chooser request was invalid.")
    default:
      return ("failed", "unknown", "Bluetooth access could not be completed.")
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
