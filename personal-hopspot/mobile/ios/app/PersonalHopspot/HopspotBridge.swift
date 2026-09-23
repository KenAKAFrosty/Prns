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

enum DiscoveryInterface: Int32 {
    case bluetoothAuto = 0
    case autoWifi = 1
}

enum DiscoveryGroupOutcome: Int32, Error {
    case applied = 0
    case unchanged = 1
    case engineUnavailable = 2
    case unsupported = 3
    case invalidInterface = 4
    case invalidEncoding = 5
    case bufferTooShort = 6
    case applyFailed = 7
    case busy = 8
}

enum DiscoveryGroupsClient {
    static func inventory(_ interface: DiscoveryInterface) -> Result<[String], DiscoveryGroupOutcome> {
        var encoded = [UInt8](
            repeating: 0,
            count: Int(hopspot_discovery_groups_wire_capacity())
        )
        let result = encoded.withUnsafeMutableBufferPointer { buffer in
            hopspot_discovery_groups(interface.rawValue, buffer.baseAddress, buffer.count)
        }
        guard result >= 0 else {
            let code = -1 - result
            return .failure(DiscoveryGroupOutcome(rawValue: code) ?? .invalidEncoding)
        }
        guard result > 0, Int(result) <= encoded.count else {
            return .failure(.invalidEncoding)
        }
        let resultLength = Int(result)
        var offset = 0
        let count = Int(encoded[offset])
        offset += 1
        var groups: [String] = []
        groups.reserveCapacity(count)
        for _ in 0..<count {
            guard offset < resultLength else { return .failure(.invalidEncoding) }
            let length = Int(encoded[offset])
            offset += 1
            guard length > 0, offset + length <= resultLength else {
                return .failure(.invalidEncoding)
            }
            let bytes = encoded[offset..<(offset + length)]
            guard let group = String(bytes: bytes, encoding: .utf8) else {
                return .failure(.invalidEncoding)
            }
            groups.append(group)
            offset += length
        }
        guard offset == resultLength else { return .failure(.invalidEncoding) }
        return .success(groups)
    }

    static func replace(
        _ interface: DiscoveryInterface,
        groups: [String]
    ) -> DiscoveryGroupOutcome {
        guard (1...4).contains(groups.count) else { return .invalidEncoding }
        let canonical = groups.map { Array($0.utf8) }.sorted {
            $0.lexicographicallyPrecedes($1)
        }
        var encoded = [UInt8]()
        encoded.reserveCapacity(Int(hopspot_discovery_groups_wire_capacity()))
        encoded.append(UInt8(canonical.count))
        for bytes in canonical {
            guard (1...32).contains(bytes.count) else { return .invalidEncoding }
            encoded.append(UInt8(bytes.count))
            encoded.append(contentsOf: bytes)
        }
        let result = encoded.withUnsafeBufferPointer { buffer in
            hopspot_replace_discovery_groups(interface.rawValue, buffer.baseAddress, buffer.count)
        }
        return DiscoveryGroupOutcome(rawValue: result) ?? .invalidEncoding
    }
}
