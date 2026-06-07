//WIP NEEDS REVIEW
import CoreGraphics
import Foundation

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
    }

    deinit {
        hopspot_free(handle)
    }

    @discardableResult
    func postInput(_ code: Int32) -> Int32 {
        hopspot_post_input(handle, code)
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
