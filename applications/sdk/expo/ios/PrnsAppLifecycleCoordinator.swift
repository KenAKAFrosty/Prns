import Foundation
import UIKit

@MainActor
final class PrnsAppLifecycleCoordinator: NSObject {
  static let shared = PrnsAppLifecycleCoordinator()

  private var observingProtectedDataTransitions = false
  private var protectedDataRecovery = PrnsProtectedDataRecovery()
  private var waitingForProtectedData = false

  private struct PreparationFailure: LocalizedError {
    let recovery: PrnsProtectedDataRecovery.Failure
    var errorDescription: String? { "Bluetooth restoration could not be prepared." }
  }

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
    // Admit the generation now, before any background preparation. This keeps
    // native launch independent of JS and coalesces a concurrent app Start.
    startNativeRuntime(application: application)
  }

  nonisolated private static func prepareRestoration() throws {
    let outcome: AppleBluetoothRestorationPreparationOutcome
    do {
      outcome = try PrnsAppModule.prepareBluetoothCentralRestoration()
    } catch {
      Self.log(.prepareBridgeFailed)
      throw PreparationFailure(recovery: .bridge)
    }
    let summary = Self.outcomeSummary(outcome)
    Self.log(
      .nativeOutcome(
        operation: .prepare,
        type: summary.type,
        stage: summary.stage
      )
    )
    if case .failed = outcome {
      throw PreparationFailure(recovery: .native(stage: summary.stage))
    }
  }

  private func startNativeRuntime(application: UIApplication) {
    let input = PrnsAppModule.configuredStartInput()

    PrnsAppModule.startAuthorized(input, prepareRestoration: Self.prepareRestoration) { result in
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
        if case .failed = outcome {
          self.recoverAfterProtectedDataFailure(
            .native(stage: summary.stage),
            application: application
          )
        } else {
          self.protectedDataRecovery.finishAttempt()
        }
      case .failure(let error):
        if let preparation = error as? PreparationFailure {
          self.recoverAfterProtectedDataFailure(preparation.recovery, application: application)
          return
        }
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

  nonisolated private static func outcomeSummary(
    _ outcome: AppleBluetoothRestorationPreparationOutcome
  ) -> (type: String, stage: String?) {
    switch outcome {
    case .prepared: return ("prepared", nil)
    case .alreadyPrepared: return ("alreadyPrepared", nil)
    case .alreadyRunning: return ("alreadyRunning", nil)
    case .failed(let stage, _): return ("failed", String(describing: stage))
    }
  }

  nonisolated private static func outcomeSummary(
    _ outcome: DevelopmentNodeStartOutcome
  ) -> (type: String, stage: String?) {
    switch outcome {
    case .started: return ("started", nil)
    case .alreadyRunning: return ("alreadyRunning", nil)
    case .failed(let stage, _): return ("failed", String(describing: stage))
    }
  }

  nonisolated private static func log(_ event: PrnsIosDiagnostics.LifecycleEvent) {
    PrnsIosDiagnostics.lifecycle(event)
  }
}
