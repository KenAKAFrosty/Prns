#if DEBUG
import Foundation

private let prnsRestorationEvents: Set<String> = [
  "central_characteristic_discovery_failed",
  "central_columba_subscribed",
  "central_connect_failed",
  "central_connected",
  "central_control_buffer_overflow",
  "central_control_subscribed",
  "central_control_subscribing",
  "central_data_buffer_overflow",
  "central_dial_failed",
  "central_dial_inbound_session_yielded",
  "central_dial_missing_peripheral",
  "central_dial_started",
  "central_dial_system_connection_yielded",
  "central_dial_timeout",
  "central_disconnected",
  "central_manager_ready",
  "central_manager_timeout",
  "central_peer_sighted",
  "central_pending_connection_resumed",
  "central_radio_disabled",
  "central_radio_enabled",
  "central_scan_restarted",
  "central_scan_started",
  "central_scan_stopped",
  "central_service_discovery_failed",
  "central_session_resumed",
  "central_state_restored",
  "central_subscription_failed",
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
  // Match the lifecycle sink so devicectl --console can capture these breadcrumbs.
  // NSLog also reaches unified logging; use one tagged sink to avoid duplicate events.
  NSLog("PRNS_IOS_RESTORATION sequence=%llu event=%@", sequence, code)
}
#endif
