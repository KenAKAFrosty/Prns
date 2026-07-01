//WIP NEEDS REVIEW
import CoreGraphics
import Foundation
import UIKit

final class HopspotBridge {
    static let inputShortPress: Int32 = 0
    static let inputLongPress: Int32 = 1
    static let actionNone: Int32 = 0
    static let actionAnnounce: Int32 = 1

    let width: Int
    let height: Int

    private let handle: OpaquePointer
    private var buffer: [UInt8]
    private let colorSpace = CGColorSpaceCreateDeviceRGB()

    init() {
        handle = hopspot_init()
        width = Int(hopspot_panel_width())
        height = Int(hopspot_panel_height())
        buffer = [UInt8](repeating: 0, count: width * height * 4)
        UIDevice.current.isBatteryMonitoringEnabled = true
    }

    deinit {
        hopspot_free(handle)
    }

    @discardableResult
    func postInput(_ code: Int32) -> Int32 {
        let action = hopspot_post_input(handle, code)
        if action == Self.actionAnnounce {
            hopspot_announce()
        }
        return action
    }

    /// Read the OS battery (level + charging) from UIKit and push it to the native face. A `.unknown`
    /// state or a negative level leaves the face on its previous reading.
    func updateBattery() {
        let level = UIDevice.current.batteryLevel
        let state = UIDevice.current.batteryState
        guard level >= 0, state != .unknown else {
            return
        }
        let percent = Int32((level * 100).rounded())
        let charging = state == .charging || state == .full
        hopspot_set_battery(handle, percent, charging)
    }

    func render() -> CGImage? {
        buffer.withUnsafeMutableBufferPointer { ptr in
            hopspot_render(handle, ptr.baseAddress, ptr.count)
        }
        guard let provider = CGDataProvider(data: Data(buffer) as CFData) else {
            return nil
        }
        return CGImage(
            width: width,
            height: height,
            bitsPerComponent: 8,
            bitsPerPixel: 32,
            bytesPerRow: width * 4,
            space: colorSpace,
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
            provider: provider,
            decode: nil,
            shouldInterpolate: false,
            intent: .defaultIntent
        )
    }
}
