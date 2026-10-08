#if canImport(PrnsHostExpo)
import PrnsHostExpo
#endif
import Foundation
import UIKit

@MainActor
final class PrnsAppLifecycleCoordinator: NSObject {
  static let shared = PrnsAppLifecycleCoordinator()

  private lazy var protectedData = PrnsProtectedDataCoordinator(
    retry: { [weak self] application in self?.prepareAndStartNativeRuntime(application: application) },
    onEvent: { event in
      switch event {
      case .availableWhileObserverInstalled: Self.log(.protectedDataAvailableWhileObserverInstalled)
      case .deferredUntilAvailable: Self.log(.startDeferredUntilProtectedDataAvailable)
      case .available: Self.log(.protectedDataAvailable)
      }
    }
  )

  private enum RecoveryFailure {
    case bridge, cancelled, native(stage: String?)
    var eligible: Bool {
      switch self {
      case .bridge: return true
      case .cancelled: return false
      case .native(let stage): return stage == "storage" || stage == "identity" || stage == "persistenceRestore"
      }
    }
  }

  private struct PreparationFailure: LocalizedError {
    let recovery: RecoveryFailure
    var errorDescription: String? { "Bluetooth restoration could not be prepared." }
  }

  func launch(application: UIApplication) {
    protectedData.observe(application: application)
    let restorationAttempt: Bool
    do {
      _ = try PrnsAppModule.restorationIdentifiers()
      // Scene-based launches provide no Bluetooth launch reason. Recreate the
      // stable dual-role owner on each eligible process launch, before any scene
      // or JavaScript. Native preparation still requires an existing identity.
      restorationAttempt = true
    } catch {
      Self.log(.configurationFailed)
      restorationAttempt = false
    }
    Self.log(
      .launch(
        restorationAttempt: restorationAttempt,
        protectedData: application.isProtectedDataAvailable
      )
    )
    PrnsBluetoothCoordinator.shared.activate(
      restorationAttemptRequested: restorationAttempt
    ) { [weak self, weak application] in
      guard let self, let application else {
        return
      }
      self.prepareAndStartNativeRuntime(application: application)
    }
  }

  func nativeStopWillBegin(application: UIApplication) {
    protectedData.stop(application: application)
  }

  private func prepareAndStartNativeRuntime(application: UIApplication) {
    do {
      try PrnsBluetoothCoordinator.shared.requireRestorationAuthorized()
    } catch {
      protectedData.finishAttempt()
      PrnsBluetoothCoordinator.shared.restorationStartAuthorizationDidClose()
      Self.log(.restorationStartRearmedAfterAuthorizationClosed)
      return
    }
    protectedData.beginAttempt(application: application)
    // Admit the generation now, before any background preparation. This keeps
    // native launch independent of JS and coalesces a concurrent app Start.
    startNativeRuntime(application: application)
  }

  nonisolated private static func prepareRestoration() throws {
    let outcome: AppleBluetoothRestorationPreparationOutcome
    do {
      outcome = try PrnsAppModule.prepareBluetoothRestoration()
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

    PrnsAppModule.startNative(
      input,
      restorationOnly: true,
      prepareRestoration: Self.prepareRestoration
    ) { result in
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
          self.protectedData.finishAttempt()
        }
      case .failure(let error):
        if let preparation = error as? PreparationFailure {
          self.recoverAfterProtectedDataFailure(preparation.recovery, application: application)
          return
        }
        Self.log(.startBridgeFailed)
        self.recoverAfterProtectedDataFailure(
          (error as? PrnsNativeStartInterruption)?.cancelsRestoration == true ? .cancelled : .bridge,
          application: application
        )
      }
    }
  }

  private func recoverAfterProtectedDataFailure(
    _ failure: RecoveryFailure,
    application: UIApplication
  ) {
    if case .cancelled = failure {
      protectedData.finishAttempt()
      return
    }
    do {
      try PrnsBluetoothCoordinator.shared.requireRestorationAuthorized()
    } catch {
      protectedData.finishAttempt()
      PrnsBluetoothCoordinator.shared.restorationStartAuthorizationDidClose()
      Self.log(.restorationStartRearmedAfterAuthorizationClosed)
      return
    }
    protectedData.recoverIfNeeded(eligible: failure.eligible, application: application)
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
