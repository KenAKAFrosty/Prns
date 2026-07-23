import SwiftUI
import UIKit

final class AppDelegate: NSObject, UIApplicationDelegate {
    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        Task { @MainActor in
            EngineController.shared.launch(options: launchOptions)
        }
        return true
    }

    func applicationWillTerminate(_ application: UIApplication) {
        MainActor.assumeIsolated {
            EngineController.shared.stopSynchronously()
        }
    }
}

@main
struct PersonalHopspotApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @StateObject private var engine = EngineController.shared

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(engine)
        }
    }
}
