import Foundation
import OSLog

enum PrnsIosDiagnostics {
  enum LifecycleOperation: String {
    case prepare
    case start
  }

  enum LifecycleEvent {
    case configurationFailed
    case launch(centralRestoration: Bool, protectedData: Bool)
    case protectedDataAvailableWhileObserverInstalled
    case startDeferredUntilProtectedDataAvailable
    case protectedDataAvailable
    case restorationStartRearmedAfterAuthorizationClosed
    case nativeOutcome(operation: LifecycleOperation, type: String, stage: String?)
    case prepareBridgeFailed
    case startBridgeFailed
  }

  enum AccessorySetupPhase: String {
    case activating
    case failed
    case ready
    case setupRequired
  }

  enum AccessoryPickerPhase: String {
    case idle
    case presented
    case presenting
  }

  enum NativeStartPhase: String {
    case failed
    case notRequested
    case running
    case starting
    case stopping
  }

  enum RestorationEvent: String {
    case centralCharacteristicDiscoveryFailed = "central_characteristic_discovery_failed"
    case centralClosedSessionReaped = "central_closed_session_reaped"
    case centralColumbaSubscribed = "central_columba_subscribed"
    case centralConnectFailed = "central_connect_failed"
    case centralConnected = "central_connected"
    case centralControlBufferOverflow = "central_control_buffer_overflow"
    case centralControlSubscribed = "central_control_subscribed"
    case centralControlSubscribing = "central_control_subscribing"
    case centralDataBufferOverflow = "central_data_buffer_overflow"
    case centralDialFailed = "central_dial_failed"
    case centralDialInboundSessionYielded = "central_dial_inbound_session_yielded"
    case centralDialMissingPeripheral = "central_dial_missing_peripheral"
    case centralDialStarted = "central_dial_started"
    case centralDialSystemConnectionYielded = "central_dial_system_connection_yielded"
    case centralDialTimeout = "central_dial_timeout"
    case centralDisconnected = "central_disconnected"
    case centralManagerReady = "central_manager_ready"
    case centralManagerTimeout = "central_manager_timeout"
    case centralPeerSighted = "central_peer_sighted"
    case centralPendingConnectionResumed = "central_pending_connection_resumed"
    case centralRadioDisabled = "central_radio_disabled"
    case centralRadioEnabled = "central_radio_enabled"
    case centralScanAlreadyScanning = "central_scan_already_scanning"
    case centralScanAlreadyStopped = "central_scan_already_stopped"
    case centralScanDecisionRestart = "central_scan_decision_restart"
    case centralScanDecisionStart = "central_scan_decision_start"
    case centralScanDecisionStop = "central_scan_decision_stop"
    case centralScanJobRadioDisabled = "central_scan_job_radio_disabled"
    case centralScanJobStarted = "central_scan_job_started"
    case centralScanRequestedOff = "central_scan_requested_off"
    case centralScanRequestedOn = "central_scan_requested_on"
    case centralScanRestarted = "central_scan_restarted"
    case centralScanStarted = "central_scan_started"
    case centralScanStateQuery = "central_scan_state_query"
    case centralScanStopped = "central_scan_stopped"
    case centralServiceDiscoveryFailed = "central_service_discovery_failed"
    case centralSessionResumed = "central_session_resumed"
    case centralStateRestored = "central_state_restored"
    case centralSubscriptionFailed = "central_subscription_failed"
    case gattControlHelloSent = "gatt_control_hello_sent"
    case gattControlWelcomeReceived = "gatt_control_welcome_received"
    case loggerInstalled = "logger_installed"
    case loggerUnavailable = "logger_unavailable"
  }

  private enum Channel: String {
    case lifecycle = "PRNS_IOS_LIFECYCLE"
    case accessorySetup = "PRNS_IOS_ASK"
    case restoration = "PRNS_IOS_RESTORATION"
  }

  private enum NativeOutcome: String {
    case alreadyPrepared
    case alreadyRunning
    case failed
    case invalid
    case prepared
    case started
    case unknown
  }

  private enum NativeStage: String {
    case bluetooth
    case contract
    case identity
    case node
    case none
    case persistenceRestore
    case runtime
    case storage
    case unknown
  }

  private static let logger = Logger(
    subsystem: Bundle.main.bundleIdentifier ?? "rs.reticulum.prns",
    category: "native-lifecycle"
  )

  static func lifecycle(_ event: LifecycleEvent) {
    let message: String
    switch event {
    case .configurationFailed:
      message = "configuration failed"
    case .launch(let centralRestoration, let protectedData):
      message = "launch centralRestoration=\(centralRestoration) protectedData=\(protectedData)"
    case .protectedDataAvailableWhileObserverInstalled:
      message = "protected data became available while the retry observer was installed"
    case .startDeferredUntilProtectedDataAvailable:
      message = "start deferred until protected data becomes available"
    case .protectedDataAvailable:
      message = "protected data became available"
    case .restorationStartRearmedAfterAuthorizationClosed:
      message = "restoration start re-armed after Bluetooth authorization closed"
    case .nativeOutcome(let operation, let type, let stage):
      let safeType = nativeOutcome(operation: operation, rawValue: type)
      let safeStage = nativeStage(operation: operation, rawValue: stage)
      message = "\(operation.rawValue) outcome=\(safeType.rawValue) stage=\(safeStage.rawValue)"
    case .prepareBridgeFailed:
      message = "prepare bridge failed"
    case .startBridgeFailed:
      message = "start bridge failed"
    }
    emit(.lifecycle, message)
  }

  static func accessorySetup(
    phase: AccessorySetupPhase,
    picker: AccessoryPickerPhase,
    authorizedCount: Int,
    nativeStart: NativeStartPhase,
    restoration: Bool
  ) {
    let nonnegativeAuthorizedCount = max(0, authorizedCount)
    emit(
      .accessorySetup,
      "phase=\(phase.rawValue) picker=\(picker.rawValue) authorized=\(nonnegativeAuthorizedCount) "
        + "nativeStart=\(nativeStart.rawValue) restoration=\(restoration)"
    )
  }

  static func restoration(sequence: UInt64, event: RestorationEvent) {
    emit(.restoration, "sequence=\(sequence) event=\(event.rawValue)")
  }

  private static func nativeOutcome(
    operation: LifecycleOperation,
    rawValue: String
  ) -> NativeOutcome {
    switch (operation, rawValue) {
    case (.prepare, "prepared"):
      return .prepared
    case (.prepare, "alreadyPrepared"):
      return .alreadyPrepared
    case (.prepare, "alreadyRunning"), (.start, "alreadyRunning"):
      return .alreadyRunning
    case (.prepare, "failed"), (.start, "failed"):
      return .failed
    case (.prepare, "invalid"), (.start, "invalid"):
      return .invalid
    case (.start, "started"):
      return .started
    default:
      return .unknown
    }
  }

  private static func nativeStage(
    operation: LifecycleOperation,
    rawValue: String?
  ) -> NativeStage {
    guard let rawValue else {
      return .none
    }
    switch (operation, rawValue) {
    case (.prepare, "contract"), (.start, "contract"):
      return .contract
    case (.prepare, "storage"), (.start, "storage"):
      return .storage
    case (.prepare, "identity"), (.start, "identity"):
      return .identity
    case (.prepare, "runtime"), (.start, "runtime"):
      return .runtime
    case (.start, "persistenceRestore"):
      return .persistenceRestore
    case (.start, "bluetooth"):
      return .bluetooth
    case (.start, "node"):
      return .node
    default:
      return .unknown
    }
  }

  // All public values are assembled above from closed enums, booleans, counts,
  // and sanitized native outcome fields. Keep the tag in the message because
  // USB log relays need not expose unified-log category metadata.
  private static func emit(_ channel: Channel, _ message: String) {
    logger.notice("\(channel.rawValue, privacy: .public) \(message, privacy: .public)")
    #if DEBUG
    // A devicectl --console session captures process stderr, not necessarily
    // unified logs. Mirror there directly, without a second NSLog/OSLog event.
    fputs("\(channel.rawValue) \(message)\n", stderr)
    #endif
  }
}
