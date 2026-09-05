import Foundation

@main
enum PrnsAppRestorationProbeTests {
  static func main() {
    emit(Array("logger_installed".utf8), sequence: 17)
    emit(Array("central_scan_already_scanning".utf8), sequence: 18)
    emit(Array("central_scan_started".utf8), sequence: UInt64.max)

    // Invalid calls must never reach the console, including private/error payloads.
    prnsAppIosRestorationProbeEmit(100, nil, 14)
    let valid = Array("logger_installed".utf8)
    valid.withUnsafeBufferPointer { buffer in
      prnsAppIosRestorationProbeEmit(101, buffer.baseAddress, 0)
      prnsAppIosRestorationProbeEmit(102, buffer.baseAddress, 65)
    }
    emit(Array("private-peer-payload".utf8), sequence: 103)
    emit(Array("logger_installed\nprivate-error".utf8), sequence: 104)
    emit([0xff, 0xfe], sequence: 105)
  }

  private static func emit(_ bytes: [UInt8], sequence: UInt64) {
    bytes.withUnsafeBufferPointer { buffer in
      prnsAppIosRestorationProbeEmit(sequence, buffer.baseAddress, UInt(buffer.count))
    }
  }
}
