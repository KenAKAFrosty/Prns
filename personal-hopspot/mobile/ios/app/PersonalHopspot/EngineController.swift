import Foundation
import OSLog
import UIKit

@MainActor
final class EngineController: ObservableObject {
    static let shared = EngineController()

    @Published private(set) var stateCode: Int32 = hopspot_engine_state()
    @Published private(set) var failureCode: Int32 = hopspot_engine_last_failure()

    private let lifecycleQueue = DispatchQueue(label: "com.personal.hopspot.engine-lifecycle")
    private let logger = Logger(subsystem: "com.personal.hopspot", category: "engine")
    private var refreshTimer: Timer?
    private var started = false
    private var lastLoggedState: Int32?
    private var lastLoggedFailure: Int32?

    private init() {}

    func launch(options: [UIApplication.LaunchOptionsKey: Any]?) {
        guard !started else { return }
        started = true

        let centralRestoration = options?[.bluetoothCentrals] != nil
        let peripheralRestoration = options?[.bluetoothPeripherals] != nil
        logger.notice(
            "HOPSPOT_IOS_RESTORATION central=\(centralRestoration, privacy: .public) peripheral=\(peripheralRestoration, privacy: .public)"
        )
        beginRefresh()

        let storagePath: String?
        do {
            let base = try FileManager.default.url(
                for: .applicationSupportDirectory,
                in: .userDomainMask,
                appropriateFor: nil,
                create: true
            )
            let directory = base.appendingPathComponent("PersonalHopspot", isDirectory: true)
            try FileManager.default.createDirectory(
                at: directory,
                withIntermediateDirectories: true
            )
            storagePath = directory.path
        } catch {
            storagePath = nil
            logger.error(
                "HOPSPOT_IOS_STORAGE state=failed error=\(String(describing: error), privacy: .public)"
            )
        }

        lifecycleQueue.async { [logger] in
            let result: Int32
            if let storagePath {
                result = storagePath.withCString { hopspot_start_engine($0) }
            } else {
                result = hopspot_start_engine(nil)
            }
            logger.notice("HOPSPOT_IOS_START result=\(result, privacy: .public)")
        }
    }

    func stopSynchronously() {
        refreshTimer?.invalidate()
        refreshTimer = nil
        let result = hopspot_stop_engine()
        started = false
        refresh()
        logger.notice("HOPSPOT_IOS_STOP result=\(result, privacy: .public)")
    }

    var isRunning: Bool {
        stateCode == Int32(HopspotEngineRunning.rawValue)
    }

    var isStarting: Bool {
        stateCode == Int32(HopspotEngineStarting.rawValue)
    }

    var isFailed: Bool {
        stateCode == Int32(HopspotEngineFailed.rawValue)
    }

    var failureDescription: String {
        switch failureCode {
        case Int32(HopspotEngineFailureStorageConfiguration.rawValue):
            "Storage configuration"
        case Int32(HopspotEngineFailureWorkerSpawn.rawValue):
            "Worker spawn"
        case Int32(HopspotEngineFailureRuntimeBuild.rawValue):
            "Runtime build"
        case Int32(HopspotEngineFailureLocalListenerBind.rawValue):
            "Local listener bind"
        case Int32(HopspotEngineFailureRpcListenerBind.rawValue):
            "RPC listener bind"
        case Int32(HopspotEngineFailureStartupTimeout.rawValue):
            "Startup timeout"
        case Int32(HopspotEngineFailureWorkerStopped.rawValue):
            "Worker stopped"
        case Int32(HopspotEngineFailureShutdownTimeout.rawValue):
            "Shutdown timeout"
        case Int32(HopspotEngineFailurePersistenceWrite.rawValue):
            "Persistence write"
        default:
            "Unknown failure"
        }
    }

    private func beginRefresh() {
        refresh()
        let timer = Timer(timeInterval: 0.25, repeats: true) { [weak self] _ in
            guard let controller = self else { return }
            Task { @MainActor in controller.refresh() }
        }
        RunLoop.main.add(timer, forMode: .common)
        refreshTimer = timer
    }

    private func refresh() {
        let state = hopspot_engine_state()
        let failure = hopspot_engine_last_failure()
        stateCode = state
        failureCode = failure
        if state != lastLoggedState || failure != lastLoggedFailure {
            logger.notice(
                "HOPSPOT_IOS_STATE state=\(state, privacy: .public) failure=\(failure, privacy: .public)"
            )
            lastLoggedState = state
            lastLoggedFailure = failure
        }
    }
}
