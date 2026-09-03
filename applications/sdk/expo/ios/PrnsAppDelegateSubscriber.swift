import ExpoModulesCore
import UIKit

public final class PrnsAppDelegateSubscriber: ExpoAppDelegateSubscriber {
  public func application(
    _ application: UIApplication,
    willFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
  ) -> Bool {
    PrnsAppLifecycleCoordinator.shared.launch(application: application, options: launchOptions)
    return true
  }
}
