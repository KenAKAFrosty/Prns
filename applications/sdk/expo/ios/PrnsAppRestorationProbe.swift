#if DEBUG
import Foundation
import OSLog

private let prnsRestorationLogger = Logger(
  subsystem: Bundle.main.bundleIdentifier ?? "rs.reticulum.prns",
  category: "PRNS_IOS_RESTORATION"
)

private let prnsRestorationEvents: Set<String> = [
  "central_control_buffer_overflow",
  "central_data_buffer_overflow",
  "central_session_resumed",
  "central_state_restored",
  "logger_installed",
  "logger_unavailable",
]

@_cdecl("prns_app_ios_restoration_probe_emit")
func prnsAppIosRestorationProbeEmit(
  _ sequence: UInt64,
  _ codePointer: UnsafePointer<UInt8>?,
  _ codeLength: UInt
) {
  guard codeLength > 0, codeLength <= 64, let codePointer else {
    return
  }
  let code = String(decoding: UnsafeBufferPointer(start: codePointer, count: Int(codeLength)), as: UTF8.self)
  guard prnsRestorationEvents.contains(code) else {
    return
  }
  prnsRestorationLogger.notice(
    "sequence=\(sequence, privacy: .public) event=\(code, privacy: .public)"
  )
}
#endif
