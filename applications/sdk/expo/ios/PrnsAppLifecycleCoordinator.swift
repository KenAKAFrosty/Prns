import Foundation
import UIKit

@MainActor
final class PrnsAppLifecycleCoordinator: NSObject {
  static let shared = PrnsAppLifecycleCoordinator()

  private var observingProtectedDataTransitions = false
  private var protectedDataRecovery = PrnsProtectedDataRecovery()
  private var waitingForProtectedData = false

  func launch(
    application: UIApplication,
    options: [UIApplication.LaunchOptionsKey: Any]?
  ) {
    observeProtectedDataTransitions(application: application)
    let centralRestoration: Bool
    do {
      let identifier = try PrnsAppModule.restorationIdentifier()
      centralRestoration = Self.restorationLaunchIdentifiers(
        options?[.bluetoothCentrals]
      ).contains(identifier)
    } catch {
      Self.log(.configurationFailed)
      centralRestoration = false
    }
    Self.log(
      .launch(
        centralRestoration: centralRestoration,
        protectedData: application.isProtectedDataAvailable
      )
    )
    PrnsAccessorySetupCoordinator.shared.activate(
      restorationLaunchRequested: centralRestoration
    ) { [weak self, weak application] in
      guard let self, let application else {
        return
      }
      self.prepareAndStartNativeRuntime(application: application)
    }
  }

  nonisolated private static func restorationLaunchIdentifiers(_ value: Any?) -> [String] {
    if let identifiers = value as? [String] {
      return identifiers
    }
    guard let identifiers = value as? NSArray else {
      return []
    }
    return identifiers.compactMap { $0 as? String }
  }

  private func observeProtectedDataTransitions(application: UIApplication) {
    guard !observingProtectedDataTransitions else {
      return
    }
    observingProtectedDataTransitions = true
    NotificationCenter.default.addObserver(
      self,
      selector: #selector(protectedDataWillBecomeUnavailable(_:)),
      name: UIApplication.protectedDataWillBecomeUnavailableNotification,
      object: application
    )
  }

  @objc
  private func protectedDataWillBecomeUnavailable(_: Notification) {
    protectedDataRecovery.protectedDataWillBecomeUnavailable()
  }

  private func waitForProtectedData(application: UIApplication) {
    guard !waitingForProtectedData else {
      return
    }
    waitingForProtectedData = true
    NotificationCenter.default.addObserver(
      self,
      selector: #selector(protectedDataDidBecomeAvailable(_:)),
      name: UIApplication.protectedDataDidBecomeAvailableNotification,
      object: application
    )
    guard !application.isProtectedDataAvailable else {
      stopWaitingForProtectedData(application: application)
      Self.log(.protectedDataAvailableWhileObserverInstalled)
      prepareAndStartNativeRuntime(application: application)
      return
    }
    Self.log(.startDeferredUntilProtectedDataAvailable)
  }

  @objc
  private func protectedDataDidBecomeAvailable(_ notification: Notification) {
    guard let application = notification.object as? UIApplication else {
      return
    }
    stopWaitingForProtectedData(application: application)
    Self.log(.protectedDataAvailable)
    prepareAndStartNativeRuntime(application: application)
  }

  private func stopWaitingForProtectedData(application: UIApplication) {
    NotificationCenter.default.removeObserver(
      self,
      name: UIApplication.protectedDataDidBecomeAvailableNotification,
      object: application
    )
    waitingForProtectedData = false
  }

  func nativeStopWillBegin(application: UIApplication) {
    if waitingForProtectedData {
      stopWaitingForProtectedData(application: application)
    }
    protectedDataRecovery.finishAttempt()
  }

  private func prepareAndStartNativeRuntime(application: UIApplication) {
    do {
      try PrnsAccessorySetupCoordinator.shared.requireAuthorized()
    } catch {
      protectedDataRecovery.finishAttempt()
      PrnsAccessorySetupCoordinator.shared.restorationStartAuthorizationDidClose()
      Self.log(.restorationStartRearmedAfterAuthorizationClosed)
      return
    }
    protectedDataRecovery.beginAttempt(
      protectedDataAvailable: application.isProtectedDataAvailable
    )
    do {
      let outcome = try PrnsAppModule.prepareBluetoothCentralRestoration()
      let summary = Self.outcomeSummary(outcome)
      Self.log(
        .nativeOutcome(
          operation: .prepare,
          type: summary.type,
          stage: summary.stage
        )
      )
      switch summary.type {
      case "prepared", "alreadyPrepared":
        startNativeRuntime(application: application)
      case "alreadyRunning":
        protectedDataRecovery.finishAttempt()
        return
      case "failed":
        recoverAfterProtectedDataFailure(
          .native(stage: summary.stage),
          application: application
        )
      default:
        protectedDataRecovery.finishAttempt()
        return
      }
    } catch {
      Self.log(.prepareBridgeFailed)
      recoverAfterProtectedDataFailure(.bridge, application: application)
    }
  }

  private func startNativeRuntime(application: UIApplication) {
    let inputJSON: String
    do {
      inputJSON = try PrnsAppModule.configuredStartInputJSON()
    } catch {
      protectedDataRecovery.finishAttempt()
      Self.log(.configurationFailed)
      return
    }

    PrnsAppModule.startAuthorized(inputJSON) { result in
      switch result {
      case .success(let outcome):
        let summary = Self.outcomeSummary(outcome)
        Self.log(
          .nativeOutcome(
            operation: .start,
            type: summary.type,
            stage: summary.stage
          )
        )
        if summary.type == "failed" {
          self.recoverAfterProtectedDataFailure(
            .native(stage: summary.stage),
            application: application
          )
        } else {
          self.protectedDataRecovery.finishAttempt()
        }
      case .failure(let error):
        Self.log(.startBridgeFailed)
        self.recoverAfterProtectedDataFailure(
          error is PrnsNativeStartInterruption ? .cancelled : .bridge,
          application: application
        )
      }
    }
  }

  private func recoverAfterProtectedDataFailure(
    _ failure: PrnsProtectedDataRecovery.Failure,
    application: UIApplication
  ) {
    if case .cancelled = failure {
      protectedDataRecovery.finishAttempt()
      return
    }
    do {
      try PrnsAccessorySetupCoordinator.shared.requireAuthorized()
    } catch {
      protectedDataRecovery.finishAttempt()
      PrnsAccessorySetupCoordinator.shared.restorationStartAuthorizationDidClose()
      Self.log(.restorationStartRearmedAfterAuthorizationClosed)
      return
    }
    guard protectedDataRecovery.action(for: failure) == .waitForAvailability else {
      return
    }
    waitForProtectedData(application: application)
  }

  nonisolated private static func outcomeSummary(_ json: String) -> (type: String, stage: String?) {
    guard
      let data = json.data(using: .utf8),
      let decoded = try? JSONSerialization.jsonObject(with: data),
      let object = decoded as? [String: Any],
      let type = object["type"] as? String
    else {
      return ("invalid", nil)
    }
    return (type, object["stage"] as? String)
  }

  nonisolated private static func log(_ event: PrnsIosDiagnostics.LifecycleEvent) {
    PrnsIosDiagnostics.lifecycle(event)
  }
}
