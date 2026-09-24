#if canImport(UIKit)
import Foundation
import UIKit

/// Owns protected-data notifications and one bounded retry. The composition
/// classifies recoverable failures and supplies the native operation to retry.
@MainActor
public final class PrnsProtectedDataCoordinator: NSObject {
  public enum Event {
    case availableWhileObserverInstalled, deferredUntilAvailable, available
  }
  private var recovery = PrnsProtectedDataRecovery()
  private var observing = false
  private var waiting = false
  private let retry: (UIApplication) -> Void
  private let onEvent: (Event) -> Void

  public init(retry: @escaping (UIApplication) -> Void, onEvent: @escaping (Event) -> Void) {
    self.retry = retry
    self.onEvent = onEvent
    super.init()
  }

  public func observe(application: UIApplication) {
    guard !observing else { return }
    observing = true
    NotificationCenter.default.addObserver(self,
      selector: #selector(protectedDataWillBecomeUnavailable(_:)),
      name: UIApplication.protectedDataWillBecomeUnavailableNotification, object: application)
  }
  @objc private func protectedDataWillBecomeUnavailable(_: Notification) {
    recovery.protectedDataWillBecomeUnavailable()
  }
  public func beginAttempt(application: UIApplication) {
    recovery.beginAttempt(protectedDataAvailable: application.isProtectedDataAvailable)
  }
  public func finishAttempt() { recovery.finishAttempt() }
  public func stop(application: UIApplication) {
    if waiting { stopWaiting(application: application) }
    recovery.finishAttempt()
  }
  public func recoverIfNeeded(eligible: Bool, application: UIApplication) {
    guard recovery.action(recoveryEligible: eligible) == .waitForAvailability else { return }
    waitForProtectedData(application: application)
  }
  private func waitForProtectedData(application: UIApplication) {
    guard !waiting else { return }
    waiting = true
    NotificationCenter.default.addObserver(self,
      selector: #selector(protectedDataDidBecomeAvailable(_:)),
      name: UIApplication.protectedDataDidBecomeAvailableNotification, object: application)
    // Install first, then recheck: unlock may win the failure-handler race.
    guard !application.isProtectedDataAvailable else {
      stopWaiting(application: application)
      onEvent(.availableWhileObserverInstalled)
      retry(application)
      return
    }
    onEvent(.deferredUntilAvailable)
  }
  @objc private func protectedDataDidBecomeAvailable(_ notification: Notification) {
    guard let application = notification.object as? UIApplication else { return }
    stopWaiting(application: application)
    onEvent(.available)
    retry(application)
  }
  private func stopWaiting(application: UIApplication) {
    NotificationCenter.default.removeObserver(self,
      name: UIApplication.protectedDataDidBecomeAvailableNotification, object: application)
    waiting = false
  }
}
#endif
