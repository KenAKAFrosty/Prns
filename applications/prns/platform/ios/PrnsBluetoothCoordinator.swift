#if canImport(PrnsHostExpo)
import PrnsHostExpo
#endif
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
final class PrnsBluetoothCoordinator {
  static let shared = PrnsBluetoothCoordinator()

  private lazy var runtime = PrnsBluetoothRuntimeCoordinator<DevelopmentNodeStartOutcome>(
    authorization: {
      switch CBManager.authorization {
      case .notDetermined: return .notDetermined
      case .restricted: return .restricted
      case .denied: return .denied
      case .allowedAlways: return .allowedAlways
      @unknown default: return .restricted
      }
    },
    applicationActive: { UIApplication.shared.applicationState == .active },
    cacheOutcome: Self.alreadyRunningOutcome,
    onStatus: { [weak self] status in
      guard let self else { return }
      NotificationCenter.default.post(name: prnsBluetoothAuthorizationStatusDidChange,
        object: nil, userInfo: ["status": self.statusJSON()])
      PrnsIosDiagnostics.bluetoothAuthorization(
        authorization: status.authorization,
        nativeStart: PrnsIosDiagnostics.NativeStartPhase(rawValue: status.nativeStart.rawValue) ?? .failed,
        restorationAttempt: status.restorationAttemptRequested
      )
    }
  )

  private init() {}

  func activate(restorationAttemptRequested: Bool, restorationReady: @escaping () -> Void) {
    runtime.activate(restorationAttemptRequested: restorationAttemptRequested, restorationReady: restorationReady)
  }
  func refreshAuthorization() { runtime.refreshAuthorization() }
  func requireRestorationAuthorized() throws {
    do { try runtime.requireRestorationAuthorized() } catch { throw Self.appError(error) }
  }
  func restorationStartAuthorizationDidClose() { runtime.restorationStartAuthorizationDidClose() }

  func statusJSON() -> String {
    let status = runtime.status
    let object: [String: Any] = [
      "authorization": status.authorization.rawValue,
      "nativeStart": status.nativeStart.rawValue,
      "restorationAttemptRequested": status.restorationAttemptRequested,
      "revision": status.revision,
    ]
    guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]),
      let json = String(data: data, encoding: .utf8) else {
      return "{\"authorization\":\"restricted\",\"nativeStart\":\"failed\",\"restorationAttemptRequested\":false,\"revision\":0}"
    }
    return json
  }

  func requestNativeStart(restorationOnly: Bool,
    _ completion: @escaping (Result<DevelopmentNodeStartOutcome, Error>) -> Void
  ) throws -> UInt64? {
    do {
      return try runtime.requestNativeStart(restorationOnly: restorationOnly) { result in
        completion(result.mapError(Self.appError))
      }
    } catch { throw Self.appError(error) }
  }
  func requireStartAllowed(startGeneration: UInt64, restorationOnly: Bool) throws {
    do { try runtime.requireStartAllowed(startGeneration: startGeneration, restorationOnly: restorationOnly) }
    catch { throw Self.appError(error) }
  }
  func nativeStartDidFinish(outcome: DevelopmentNodeStartOutcome, startGeneration: UInt64) {
    runtime.nativeStartDidFinish(outcome: outcome, startGeneration: startGeneration)
  }
  func nativeStartDidFail(_ error: Error, startGeneration: UInt64) {
    runtime.nativeStartDidFail(error, startGeneration: startGeneration)
  }
  func nativeStopWillBegin() { runtime.nativeStopWillBegin() }
  func nativeStopDidFinish(outcome: DevelopmentNodeStopOutcome) {
    switch outcome {
    case .alreadyStopped, .stopped: runtime.nativeStopDidFinish(joined: true)
    default: runtime.nativeStopDidFinish(joined: false)
    }
  }

  private static func appError(_ error: Error) -> Error {
    guard let admission = error as? PrnsNativeStartAdmissionError else { return error }
    switch admission {
    case .stopInProgress: return PrnsNativeStartInterruption.stopInProgress
    case .supersededByStop: return PrnsNativeStartInterruption.supersededByStop
    case .bluetoothPermissionNeedsForeground: return PrnsNativeStartInterruption.bluetoothPermissionNeedsForeground
    case .restorationNotAuthorized: return PrnsNativeStartInterruption.restorationNotAuthorized
    case .resultUnavailable: return PrnsAppException("The native start result is unavailable.")
    case .tooManyWaiters: return PrnsAppException("Too many callers are waiting for native startup.")
    }
  }
  private static func alreadyRunningOutcome(_ outcome: DevelopmentNodeStartOutcome) -> DevelopmentNodeStartOutcome? {
    switch outcome {
    case .started(let snapshot), .alreadyRunning(let snapshot): return .alreadyRunning(snapshot: snapshot)
    case .failed: return nil
    }
  }
}
