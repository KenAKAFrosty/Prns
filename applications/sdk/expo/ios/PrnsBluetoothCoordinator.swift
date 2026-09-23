import CoreBluetooth
import Foundation
import UIKit

let prnsBluetoothAuthorizationStatusDidChange = Notification.Name(
  "rs.reticulum.prns.app.bluetooth-authorization-status-did-change"
)

enum PrnsNativeStartInterruption: LocalizedError {
  case stopInProgress
  case supersededByStop
  case bluetoothPermissionNeedsForeground
  case restorationNotAuthorized

  var cancelsRestoration: Bool {
    switch self {
    case .stopInProgress, .supersededByStop: return true
    case .bluetoothPermissionNeedsForeground, .restorationNotAuthorized: return false
    }
  }

  var errorDescription: String? {
    switch self {
    case .stopInProgress:
      return "The native node is still stopping. Reset app data before starting it again."
    case .supersededByStop:
      return "The native start request was superseded by a stop request."
    case .bluetoothPermissionNeedsForeground:
      return "Open prns to choose whether it can use Bluetooth."
    case .restorationNotAuthorized:
      return "Background Bluetooth restoration requires Bluetooth access."
    }
  }
}

@MainActor
final class PrnsBluetoothCoordinator: NSObject {
  static let shared = PrnsBluetoothCoordinator()

  private typealias NativeStartPhase = PrnsIosDiagnostics.NativeStartPhase

  private var observingAuthorization = false
  private var authorization = PrnsBluetoothAuthorization.notDetermined
  private var nativeStartPhase = NativeStartPhase.notRequested
  private var nativeStartCompletions: [(Result<DevelopmentNodeStartOutcome, Error>) -> Void] = []
  private var nativeStartGeneration: UInt64 = 0
  private var completedStartOutcome: DevelopmentNodeStartOutcome?
  private var restorationDispatch = PrnsRestorationDispatch()
  private var restorationReady: (() -> Void)?
  private var statusRevision: UInt64 = 0

  private override init() {}

  func activate(
    restorationAttemptRequested: Bool,
    restorationReady: @escaping () -> Void
  ) {
    restorationDispatch.request(restorationAttemptRequested)
    self.restorationReady = restorationReady
    if !observingAuthorization {
      observingAuthorization = true
      NotificationCenter.default.addObserver(
        self,
        selector: #selector(applicationDidBecomeActive(_:)),
        name: UIApplication.didBecomeActiveNotification,
        object: nil
      )
    }
    refreshAuthorization()
    publish()
    dispatchRestorationStartIfReady()
  }

  @objc
  private func applicationDidBecomeActive(_: Notification) {
    // Settings and the system permission alert can change authorization while
    // JavaScript is absent. Never create a second manager just to observe it.
    refreshAuthorization()
    dispatchRestorationStartIfReady()
  }

  func refreshAuthorization() {
    let current: PrnsBluetoothAuthorization
    switch CBManager.authorization {
    case .notDetermined: current = .notDetermined
    case .restricted: current = .restricted
    case .denied: current = .denied
    case .allowedAlways: current = .allowedAlways
    @unknown default: current = .restricted
    }
    guard authorization != current else { return }
    authorization = current
    publish()
  }

  func requireRestorationAuthorized() throws {
    refreshAuthorization()
    guard authorization.permitsAutomaticRestoration else {
      throw PrnsNativeStartInterruption.restorationNotAuthorized
    }
  }

  func restorationStartAuthorizationDidClose() {
    restorationDispatch.rearmAfterAuthorizationLoss()
    publish()
    dispatchRestorationStartIfReady()
  }

  func statusJSON() -> String {
    let object: [String: Any] = [
      "authorization": authorization.rawValue,
      "nativeStart": nativeStartPhase.rawValue,
      "restorationAttemptRequested": restorationDispatch.attemptRequested,
      "revision": statusRevision,
    ]
    guard
      let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]),
      let json = String(data: data, encoding: .utf8)
    else {
      return "{\"authorization\":\"restricted\",\"nativeStart\":\"failed\",\"restorationAttemptRequested\":false,\"revision\":0}"
    }
    return json
  }

  func requestNativeStart(
    restorationOnly: Bool,
    _ completion: @escaping (Result<DevelopmentNodeStartOutcome, Error>) -> Void
  ) throws -> UInt64? {
    try requireStartAllowed(restorationOnly: restorationOnly)
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

  func requireStartAllowed(startGeneration: UInt64, restorationOnly: Bool) throws {
    guard
      nativeStartGeneration == startGeneration,
      nativeStartPhase == .starting
    else {
      throw PrnsNativeStartInterruption.supersededByStop
    }
    try requireStartAllowed(restorationOnly: restorationOnly)
  }

  private func requireStartAllowed(restorationOnly: Bool) throws {
    refreshAuthorization()
    guard authorization.permitsStart(
      restorationOnly: restorationOnly,
      applicationActive: UIApplication.shared.applicationState == .active
    ) else {
      throw restorationOnly
        ? PrnsNativeStartInterruption.restorationNotAuthorized
        : PrnsNativeStartInterruption.bluetoothPermissionNeedsForeground
    }
  }

  func nativeStartDidFinish(outcome: DevelopmentNodeStartOutcome, startGeneration: UInt64) {
    guard
      nativeStartGeneration == startGeneration,
      nativeStartPhase == .starting
    else {
      return
    }
    let cachedOutcome = Self.alreadyRunningOutcome(outcome)
    nativeStartPhase = cachedOutcome == nil ? .failed : .running
    completedStartOutcome = cachedOutcome
    let completions = nativeStartCompletions
    nativeStartCompletions.removeAll(keepingCapacity: false)
    for completion in completions {
      completion(.success(outcome))
    }
    refreshAuthorization()
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
    refreshAuthorization()
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

  func nativeStopDidFinish(outcome: DevelopmentNodeStopOutcome) {
    switch outcome {
    case .alreadyStopped, .stopped:
      nativeStartPhase = .notRequested
    default:
      // A failed stop may leave the Rust supervisor stopping. Require a
      // definitive stop or reset before admitting another generation.
      nativeStartPhase = .stopping
    }
    completedStartOutcome = nil
    publish()
  }

  private func dispatchRestorationStartIfReady() {
    guard
      let restorationReady,
      restorationDispatch.claimIfAuthorized(authorization.permitsAutomaticRestoration)
    else {
      return
    }
    restorationReady()
  }

  private func publish() {
    statusRevision &+= 1
    let status = statusJSON()
    NotificationCenter.default.post(
      name: prnsBluetoothAuthorizationStatusDidChange,
      object: nil,
      userInfo: ["status": status]
    )
    PrnsIosDiagnostics.bluetoothAuthorization(
      authorization: authorization,
      nativeStart: nativeStartPhase,
      restorationAttempt: restorationDispatch.attemptRequested
    )
  }

  private static func alreadyRunningOutcome(
    _ outcome: DevelopmentNodeStartOutcome
  ) -> DevelopmentNodeStartOutcome? {
    switch outcome {
    case .started(let snapshot), .alreadyRunning(let snapshot):
      return .alreadyRunning(snapshot: snapshot)
    case .failed:
      return nil
    }
  }
}
