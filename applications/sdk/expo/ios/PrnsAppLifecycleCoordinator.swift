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
    let centralRestoration = options?[.bluetoothCentrals] != nil
    let peripheralRestoration = options?[.bluetoothPeripherals] != nil
    Self.log(
      "launch centralRestoration=\(centralRestoration) peripheralRestoration=\(peripheralRestoration) protectedData=\(application.isProtectedDataAvailable)"
    )
    guard centralRestoration || peripheralRestoration else {
      return
    }
    startNativeRuntime(application: application)
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
    startNativeRuntime(application: application)
  }

  private func startNativeRuntime(application: UIApplication) {
    let inputJSON: String
    do {
      inputJSON = try PrnsAppModule.configuredStartInputJSON()
    } catch {
      Self.log("configuration failed")
      return
    }

    PrnsAppModule.nativeQueue.async(flags: .barrier) {
      do {
        let outcome = try PrnsAppModule.startWithRestoration(inputJSON)
        let summary = Self.outcomeSummary(outcome)
        Self.log("start outcome=\(summary.type) stage=\(summary.stage ?? "none")")
        if summary.type == "failed" {
          DispatchQueue.main.async {
            self.retryAfterProtectedDataIfNeeded(application: application)
          }
        }
      } catch {
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
