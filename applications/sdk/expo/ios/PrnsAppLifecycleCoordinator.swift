import Foundation
import UIKit

@MainActor
final class PrnsAppLifecycleCoordinator: NSObject {
  static let shared = PrnsAppLifecycleCoordinator()

  private var waitingForProtectedData = false

  func launch(
    application: UIApplication,
    options: [UIApplication.LaunchOptionsKey: Any]?
  ) {
    let centralRestoration: Bool
    do {
      let identifier = try PrnsAppModule.restorationIdentifier()
      centralRestoration = Self.restorationLaunchIdentifiers(
        options?[.bluetoothCentrals]
      ).contains(identifier)
    } catch {
      Self.log("configuration failed")
      centralRestoration = false
    }
    Self.log(
      "launch centralRestoration=\(centralRestoration) protectedData=\(application.isProtectedDataAvailable)"
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
    Self.log("start deferred until protected data becomes available")
  }

  @objc
  private func protectedDataDidBecomeAvailable(_ notification: Notification) {
    NotificationCenter.default.removeObserver(
      self,
      name: UIApplication.protectedDataDidBecomeAvailableNotification,
      object: notification.object
    )
    waitingForProtectedData = false
    Self.log("protected data became available")
    guard let application = notification.object as? UIApplication else {
      return
    }
    prepareAndStartNativeRuntime(application: application)
  }

  private func prepareAndStartNativeRuntime(application: UIApplication) {
    do {
      let outcome = try PrnsAppModule.prepareBluetoothCentralRestoration()
      let summary = Self.outcomeSummary(outcome)
      Self.log("prepare outcome=\(summary.type) stage=\(summary.stage ?? "none")")
      switch summary.type {
      case "prepared", "alreadyPrepared":
        startNativeRuntime(application: application)
      case "alreadyRunning":
        return
      default:
        retryAfterProtectedDataIfNeeded(application: application)
      }
    } catch {
      Self.log("prepare bridge failed")
      retryAfterProtectedDataIfNeeded(application: application)
    }
  }

  private func startNativeRuntime(application: UIApplication) {
    let inputJSON: String
    do {
      inputJSON = try PrnsAppModule.configuredStartInputJSON()
    } catch {
      Self.log("configuration failed")
      return
    }

    PrnsAppModule.startAuthorized(inputJSON) { result in
      switch result {
      case .success(let outcome):
        let summary = Self.outcomeSummary(outcome)
        Self.log("start outcome=\(summary.type) stage=\(summary.stage ?? "none")")
        if summary.type == "failed" {
          DispatchQueue.main.async {
            self.retryAfterProtectedDataIfNeeded(application: application)
          }
        }
      case .failure:
        Self.log("start bridge failed")
        DispatchQueue.main.async {
          self.retryAfterProtectedDataIfNeeded(application: application)
        }
      }
    }
  }

  private func retryAfterProtectedDataIfNeeded(application: UIApplication) {
    guard !application.isProtectedDataAvailable else {
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

  nonisolated private static func log(_ message: String) {
    NSLog("PRNS_IOS_LIFECYCLE %@", message)
  }
}
