import Foundation

@main
enum PrnsAppRestorationProbeTests {
  static func main() {
    PrnsIosDiagnostics.lifecycle(
      .launch(centralRestoration: true, protectedData: false)
    )
    PrnsIosDiagnostics.accessorySetup(
      phase: .ready,
      picker: .idle,
      authorizedCount: 1,
      nativeStart: .running,
      restoration: true
    )
    PrnsIosDiagnostics.lifecycle(
      .nativeOutcome(operation: .prepare, type: "prepared", stage: nil)
    )
    PrnsIosDiagnostics.lifecycle(
      .nativeOutcome(operation: .start, type: "failed", stage: "runtime")
    )
    PrnsIosDiagnostics.lifecycle(
      .nativeOutcome(
        operation: .prepare,
        type: "private-outcome-must-not-be-logged",
        stage: "private-stage-must-not-be-logged"
      )
    )
    emit(Array("logger_installed".utf8), sequence: 17)
    emit(Array("central_scan_already_scanning".utf8), sequence: 18)
    emit(Array("gatt_control_hello_sent".utf8), sequence: 19)
    emit(Array("gatt_control_welcome_received".utf8), sequence: 20)
    emit(Array("central_closed_session_reaped".utf8), sequence: 21)
    emit(Array("central_restored_native_reset_requested".utf8), sequence: 22)
    emit(Array("central_restored_native_reconnect_requested".utf8), sequence: 23)
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
    emit(Array("gatt_control_hello_sent private-peer-payload".utf8), sequence: 106)
    emit(Array("gatt_control_timeout".utf8), sequence: 107)
  }

  private static func emit(_ bytes: [UInt8], sequence: UInt64) {
    bytes.withUnsafeBufferPointer { buffer in
      prnsAppIosRestorationProbeEmit(sequence, buffer.baseAddress, buffer.count)
    }
  }
}
