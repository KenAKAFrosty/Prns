import Combine
import CoreGraphics
import Foundation
import OSLog
import UIKit

@MainActor
final class HopspotBridge: ObservableObject {
    let width: Int
    let height: Int
    let renderInterval: TimeInterval

    private let handle: OpaquePointer
    private var buffer: [UInt8]
    private let bytesPerRow: Int
    private let colorSpace = CGColorSpaceCreateDeviceRGB()
    private let logger = Logger(subsystem: "com.personal.hopspot", category: "telemetry")
    private var batteryObservers: [NSObjectProtocol] = []

    init() {
        handle = hopspot_init()
        width = Int(hopspot_panel_width())
        height = Int(hopspot_panel_height())
        let rgbaBytes = Int(hopspot_rgba_bytes())
        buffer = [UInt8](repeating: 0, count: rgbaBytes)
        bytesPerRow = rgbaBytes / height
        renderInterval = TimeInterval(hopspot_render_interval_millis()) / 1_000
        startBatteryDelivery()
    }

    deinit {
        MainActor.assumeIsolated {
            for observer in batteryObservers {
                NotificationCenter.default.removeObserver(observer)
            }
            UIDevice.current.isBatteryMonitoringEnabled = false
        }
        hopspot_free(handle)
    }

    @discardableResult
    func postShortPress() -> Int32 {
        postInput(Int32(HopspotInputShortPress.rawValue))
    }

    @discardableResult
    func postLongPress() -> Int32 {
        postInput(Int32(HopspotInputLongPress.rawValue))
    }

    func render() -> CGImage? {
        buffer.withUnsafeMutableBufferPointer { pointer in
            hopspot_render(handle, pointer.baseAddress, pointer.count)
        }
        guard let provider = CGDataProvider(data: Data(buffer) as CFData) else {
            return nil
        }
        return CGImage(
            width: width,
            height: height,
            bitsPerComponent: 8,
            bitsPerPixel: 32,
            bytesPerRow: bytesPerRow,
            space: colorSpace,
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
            provider: provider,
            decode: nil,
            shouldInterpolate: false,
            intent: .defaultIntent
        )
    }

    private func postInput(_ code: Int32) -> Int32 {
        let action = hopspot_post_input(handle, code)
        if action == Int32(HopspotActionAnnounce.rawValue) {
            hopspot_announce()
        }
        return action
    }

    private func startBatteryDelivery() {
        UIDevice.current.isBatteryMonitoringEnabled = true
        let center = NotificationCenter.default
        batteryObservers = [
            center.addObserver(
                forName: UIDevice.batteryLevelDidChangeNotification,
                object: nil,
                queue: .main
            ) { [weak self] _ in
                guard let bridge = self else { return }
                Task { @MainActor in bridge.recordTelemetry(updateRenderer: true) }
            },
            center.addObserver(
                forName: UIDevice.batteryStateDidChangeNotification,
                object: nil,
                queue: .main
            ) { [weak self] _ in
                guard let bridge = self else { return }
                Task { @MainActor in bridge.recordTelemetry(updateRenderer: true) }
            },
            center.addObserver(
                forName: ProcessInfo.thermalStateDidChangeNotification,
                object: nil,
                queue: .main
            ) { [weak self] _ in
                guard let bridge = self else { return }
                Task { @MainActor in bridge.recordTelemetry(updateRenderer: false) }
            },
        ]
        recordTelemetry(updateRenderer: true)
    }

    private func recordTelemetry(updateRenderer: Bool) {
        let level = UIDevice.current.batteryLevel
        let state = UIDevice.current.batteryState
        guard level >= 0, state != .unknown else { return }
        let percent = Int32((level * 100).rounded())
        let externallyPowered = state == .charging || state == .full
        if updateRenderer {
            hopspot_set_battery(handle, percent, externallyPowered)
        }
        logger.notice(
            "HOPSPOT_IOS_TELEMETRY percent=\(percent, privacy: .public) externally_powered=\(externallyPowered, privacy: .public) thermal=\(ProcessInfo.processInfo.thermalState.rawValue, privacy: .public)"
        )
    }
}
