import ExpoModulesCore
import UIKit

public final class PrnsAppPlatformAppDelegate: ExpoAppDelegateSubscriber {
  public func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
  ) -> Bool {
    MainActor.assumeIsolated {
      let restored = launchOptions?[.bluetoothCentrals] != nil ||
        launchOptions?[.bluetoothPeripherals] != nil
      PrnsAppRuntime.shared.activate(restorationRequested: restored)
    }
    return true
  }
}
