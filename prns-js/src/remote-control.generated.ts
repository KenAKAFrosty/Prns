// Generated from public prns-core Remote Control declarations. Do not edit.
import type { IdentityConfig } from "./contract.generated.js";
export const REMOTE_CONTROL_SEMANTIC_FINGERPRINT = "becd93f1924c3ec2e50562f2df71a57b80c6cffb81f20399e616abbdc4e3df5c";
export type RemoteControlRequest =
  | { readonly tag: "Describe" }
  | { readonly tag: "AnnounceSelf" }
  | { readonly tag: "InventoryInterfaces"; readonly data: { readonly page: RemoteControlInterfacePage } }
  | { readonly tag: "SetInterfacePower"; readonly data: { readonly id: RemoteControlInterfaceId; readonly power: RemoteControlInterfacePower } }
  | { readonly tag: "SetInterfaceMode"; readonly data: { readonly id: RemoteControlInterfaceId; readonly mode: RemoteControlInterfaceMode } }
  | { readonly tag: "SetInterfaceGroup"; readonly data: { readonly id: RemoteControlInterfaceId; readonly group: RemoteControlInterfaceGroup } }
  | { readonly tag: "InventoryInterfaceDiscoveryGroups"; readonly data: { readonly id: RemoteControlInterfaceId } }
  | { readonly tag: "ReplaceInterfaceDiscoveryGroups"; readonly data: { readonly id: RemoteControlInterfaceId; readonly groups: RemoteControlDiscoveryGroups } }
  | { readonly tag: "InventoryInterfacePeers"; readonly data: { readonly id: RemoteControlInterfaceId; readonly page: RemoteControlPeerPage } }
  | { readonly tag: "InventoryInterfaceConfig"; readonly data: { readonly id: RemoteControlInterfaceId } }
  | { readonly tag: "SetInterfaceLoRaProfile"; readonly data: { readonly id: RemoteControlInterfaceId; readonly profile: RemoteControlLoRaProfile } }
  | { readonly tag: "SetInterfaceWifiStation"; readonly data: { readonly id: RemoteControlInterfaceId; readonly station: RemoteControlWifiStation } }
  | { readonly tag: "InventoryControllers"; readonly data: { readonly page: RemoteControlControllerPage } }
  | { readonly tag: "AuthorizeController"; readonly data: { readonly controller: RemoteControlControllerIdentity; readonly permittedRequests: RemoteControlRequestSet } }
  | { readonly tag: "RevokeController"; readonly data: { readonly hash: RemoteControlIdentityHash } }
  | { readonly tag: "DescribeBuild" }
  | { readonly tag: "DescribePower" }
  | { readonly tag: "SleepRadios" }
  | { readonly tag: "WakeRadios" }
  | { readonly tag: "SetSystemPower"; readonly data: { readonly power: RemoteControlSystemPower } }
  | { readonly tag: "SetGnssPower"; readonly data: { readonly power: RemoteControlGnssPower } }
  | { readonly tag: "SetDisplayVisibility"; readonly data: { readonly visibility: RemoteControlDisplayVisibility } }
  | { readonly tag: "SetDisplayAutoOff"; readonly data: { readonly autoOff: RemoteControlDisplayAutoOff } }
  | { readonly tag: "SetStationUplink"; readonly data: { readonly id: RemoteControlInterfaceId; readonly uplink: RemoteControlStationUplink } }
  | { readonly tag: "SetEspRadioMode"; readonly data: { readonly mode: RemoteControlEspRadioMode } }
  | { readonly tag: "StageWifiCredentials"; readonly data: { readonly station: RemoteControlWifiStation } }
  | { readonly tag: "ActivateWifiCredentials"; readonly data: { readonly revision: RemoteControlWifiCredentialRevision } }
  | { readonly tag: "ConfirmWifiCredentials"; readonly data: { readonly revision: RemoteControlWifiCredentialRevision } }
  | { readonly tag: "CancelWifiCredentials"; readonly data: { readonly revision: RemoteControlWifiCredentialRevision } }
  | { readonly tag: "InspectWifiTransaction" };
export type RemoteControlInterfacePage =
  | { readonly tag: "First" }
  | { readonly tag: "After"; readonly data: { readonly value: RemoteControlInterfaceCursor } };
export type RemoteControlInterfaceCursor = { readonly value: RemoteControlInterfaceId };
export type RemoteControlInterfaceId = { readonly value: Uint8Array };
export type RemoteControlInterfacePower =
  | { readonly tag: "Off" }
  | { readonly tag: "On" };
export type RemoteControlInterfaceMode =
  | { readonly tag: "Full" }
  | { readonly tag: "PointToPoint" }
  | { readonly tag: "AccessPoint" }
  | { readonly tag: "Roaming" }
  | { readonly tag: "Boundary" }
  | { readonly tag: "Gateway" }
  | { readonly tag: "Internal" };
export type RemoteControlInterfaceGroup = { readonly value: string };
export type RemoteControlDiscoveryGroups = { readonly groups: ReadonlyArray<string> };
export type RemoteControlPeerPage =
  | { readonly tag: "First" }
  | { readonly tag: "After"; readonly data: { readonly value: RemoteControlPeerCursor } };
export type RemoteControlPeerCursor = { readonly value: RemoteControlInterfaceId };
export type RemoteControlLoRaProfile = { readonly value: string };
export type RemoteControlWifiStation = { readonly ssid: string; readonly password: string };
export type RemoteControlControllerPage =
  | { readonly tag: "First" }
  | { readonly tag: "After"; readonly data: { readonly value: RemoteControlControllerCursor } };
export type RemoteControlControllerCursor = { readonly value: RemoteControlIdentityHash };
export type RemoteControlIdentityHash = { readonly value: Uint8Array };
export type RemoteControlControllerIdentity = { readonly publicKeys: Uint8Array };
export type RemoteControlRequestSet = { readonly kinds: ReadonlyArray<RemoteControlRequestKind> };
export type RemoteControlRequestKind =
  | { readonly tag: "Describe" }
  | { readonly tag: "AnnounceSelf" }
  | { readonly tag: "InventoryInterfaces" }
  | { readonly tag: "SetInterfacePower" }
  | { readonly tag: "SleepRadios" }
  | { readonly tag: "WakeRadios" }
  | { readonly tag: "SetInterfaceMode" }
  | { readonly tag: "SetInterfaceGroup" }
  | { readonly tag: "InventoryInterfacePeers" }
  | { readonly tag: "InventoryInterfaceConfig" }
  | { readonly tag: "SetInterfaceLoRaProfile" }
  | { readonly tag: "DescribeBuild" }
  | { readonly tag: "SetInterfaceWifiStation" }
  | { readonly tag: "InventoryControllers" }
  | { readonly tag: "AuthorizeController" }
  | { readonly tag: "RevokeController" }
  | { readonly tag: "DescribePower" }
  | { readonly tag: "SetSystemPower" }
  | { readonly tag: "SetGnssPower" }
  | { readonly tag: "SetDisplayVisibility" }
  | { readonly tag: "SetDisplayAutoOff" }
  | { readonly tag: "SetStationUplink" }
  | { readonly tag: "SetEspRadioMode" }
  | { readonly tag: "StageWifiCredentials" }
  | { readonly tag: "ActivateWifiCredentials" }
  | { readonly tag: "ConfirmWifiCredentials" }
  | { readonly tag: "CancelWifiCredentials" }
  | { readonly tag: "InspectWifiTransaction" }
  | { readonly tag: "InventoryInterfaceDiscoveryGroups" }
  | { readonly tag: "ReplaceInterfaceDiscoveryGroups" };
export type RemoteControlSystemPower =
  | { readonly tag: "Awake" }
  | { readonly tag: "Asleep" };
export type RemoteControlGnssPower =
  | { readonly tag: "Off" }
  | { readonly tag: "On" };
export type RemoteControlDisplayVisibility =
  | { readonly tag: "Hidden" }
  | { readonly tag: "Visible" };
export type RemoteControlDisplayAutoOff =
  | { readonly tag: "Disabled" }
  | { readonly tag: "Enabled" };
export type RemoteControlStationUplink =
  | { readonly tag: "Disabled" }
  | { readonly tag: "Enabled" };
export type RemoteControlEspRadioMode =
  | { readonly tag: "Bluetooth" }
  | { readonly tag: "AccessPoint" };
export type RemoteControlWifiCredentialRevision = { readonly value: number };
export type RemoteControlResponse =
  | { readonly tag: "Describe"; readonly data: { readonly value: RemoteControlDescription } }
  | { readonly tag: "AnnounceSelf"; readonly data: { readonly value: RemoteControlAnnounceSelfOutcome } }
  | { readonly tag: "InventoryInterfaces"; readonly data: { readonly value: RemoteControlInterfaceInventory } }
  | { readonly tag: "SetInterfacePower"; readonly data: { readonly value: RemoteControlPowerOutcome } }
  | { readonly tag: "SetInterfaceMode"; readonly data: { readonly value: RemoteControlModeOutcome } }
  | { readonly tag: "SetInterfaceGroup"; readonly data: { readonly value: RemoteControlGroupOutcome } }
  | { readonly tag: "InventoryInterfaceDiscoveryGroups"; readonly data: { readonly value: RemoteControlDiscoveryGroupsInventoryOutcome } }
  | { readonly tag: "ReplaceInterfaceDiscoveryGroups"; readonly data: { readonly value: RemoteControlDiscoveryGroupsReplaceOutcome } }
  | { readonly tag: "InventoryInterfacePeers"; readonly data: { readonly value: RemoteControlInterfacePeersOutcome } }
  | { readonly tag: "InventoryInterfaceConfig"; readonly data: { readonly value: RemoteControlInterfaceConfigOutcome } }
  | { readonly tag: "SetInterfaceLoRaProfile"; readonly data: { readonly value: RemoteControlLoRaOutcome } }
  | { readonly tag: "SetInterfaceWifiStation"; readonly data: { readonly value: RemoteControlWifiStationOutcome } }
  | { readonly tag: "InventoryControllers"; readonly data: { readonly value: RemoteControlControllerInventory } }
  | { readonly tag: "AuthorizeController"; readonly data: { readonly value: RemoteControlAuthorizeControllerOutcome } }
  | { readonly tag: "RevokeController"; readonly data: { readonly value: RemoteControlRevokeControllerOutcome } }
  | { readonly tag: "DescribeBuild"; readonly data: { readonly value: RemoteControlBuildVersion } }
  | { readonly tag: "DescribePower"; readonly data: { readonly value: RemoteControlPowerSnapshot } }
  | { readonly tag: "SleepRadios"; readonly data: { readonly value: RemoteControlSleepOutcome } }
  | { readonly tag: "WakeRadios"; readonly data: { readonly value: RemoteControlSleepOutcome } }
  | { readonly tag: "SetSystemPower"; readonly data: { readonly value: RemoteControlApplyOutcome } }
  | { readonly tag: "SetGnssPower"; readonly data: { readonly value: RemoteControlApplyOutcome } }
  | { readonly tag: "SetDisplayVisibility"; readonly data: { readonly value: RemoteControlApplyOutcome } }
  | { readonly tag: "SetDisplayAutoOff"; readonly data: { readonly value: RemoteControlApplyOutcome } }
  | { readonly tag: "SetStationUplink"; readonly data: { readonly value: RemoteControlApplyOutcome } }
  | { readonly tag: "SetEspRadioMode"; readonly data: { readonly value: RemoteControlApplyOutcome } }
  | { readonly tag: "StageWifiCredentials"; readonly data: { readonly value: RemoteControlWifiStageOutcome } }
  | { readonly tag: "ActivateWifiCredentials"; readonly data: { readonly value: RemoteControlApplyOutcome } }
  | { readonly tag: "ConfirmWifiCredentials"; readonly data: { readonly value: RemoteControlApplyOutcome } }
  | { readonly tag: "CancelWifiCredentials"; readonly data: { readonly value: RemoteControlApplyOutcome } }
  | { readonly tag: "InspectWifiTransaction"; readonly data: { readonly value: RemoteControlWifiTransactionStatus } }
  | { readonly tag: "ProtocolError"; readonly data: { readonly value: RemoteControlProtocolError } };
export type RemoteControlDescription = { readonly availableRequests: RemoteControlRequestSet };
export type RemoteControlAnnounceSelfOutcome =
  | { readonly tag: "Announced" }
  | { readonly tag: "Unavailable" }
  | { readonly tag: "Rejected" }
  | { readonly tag: "WriteFailed" };
export type RemoteControlInterfaceInventory = { readonly entries: ReadonlyArray<RemoteControlInterfaceEntry>; readonly continuation: RemoteControlInterfaceContinuation };
export type RemoteControlInterfaceEntry = { readonly id: RemoteControlInterfaceId; readonly kind: RemoteControlInterfaceKind; readonly mode: RemoteControlInterfaceMode; readonly connection: RemoteControlConnectionState; readonly enabled: boolean; readonly txBytes: bigint; readonly rxBytes: bigint; readonly links: number; readonly rateBytesPerSec: number };
export type RemoteControlInterfaceKind =
  | { readonly tag: "Loopback" }
  | { readonly tag: "TcpClient" }
  | { readonly tag: "TcpServer" }
  | { readonly tag: "Udp" }
  | { readonly tag: "Serial" }
  | { readonly tag: "UsbAutoHost" }
  | { readonly tag: "UsbAutoDevice" }
  | { readonly tag: "AutoWifi" }
  | { readonly tag: "WifiPeer" }
  | { readonly tag: "LocalServer" }
  | { readonly tag: "LocalClient" }
  | { readonly tag: "TcpServerPeer" }
  | { readonly tag: "BluetoothAuto" }
  | { readonly tag: "BluetoothPeer" }
  | { readonly tag: "LoRa" }
  | { readonly tag: "Kiss" }
  | { readonly tag: "Ax25Kiss" }
  | { readonly tag: "Pipe" }
  | { readonly tag: "Rnode" }
  | { readonly tag: "BackboneServer" }
  | { readonly tag: "BackboneServerPeer" }
  | { readonly tag: "BackboneClient" }
  | { readonly tag: "EspNow" }
  | { readonly tag: "WebSocketClient" }
  | { readonly tag: "WebSocketServer" }
  | { readonly tag: "WebSocketServerPeer" }
  | { readonly tag: "WifiDirect" }
  | { readonly tag: "WifiDirectPeer" }
  | { readonly tag: "WifiAware" }
  | { readonly tag: "WifiAwarePeer" }
  | { readonly tag: "I2p" }
  | { readonly tag: "I2pPeer" }
  | { readonly tag: "Weave" }
  | { readonly tag: "WeavePeer" };
export type RemoteControlConnectionState =
  | { readonly tag: "Initializing" }
  | { readonly tag: "Connected" }
  | { readonly tag: "Degraded" }
  | { readonly tag: "Reconnecting" }
  | { readonly tag: "Failed" }
  | { readonly tag: "Disconnected" }
  | { readonly tag: "Disabled" }
  | { readonly tag: "Unknown" };
export type RemoteControlInterfaceContinuation =
  | { readonly tag: "Complete" }
  | { readonly tag: "More"; readonly data: { readonly value: RemoteControlInterfaceCursor } };
export type RemoteControlPowerOutcome =
  | { readonly tag: "Applied" }
  | { readonly tag: "UnknownInterface" }
  | { readonly tag: "Failed" }
  | { readonly tag: "Unchanged" }
  | { readonly tag: "Scheduled" };
export type RemoteControlModeOutcome =
  | { readonly tag: "Applied" }
  | { readonly tag: "UnknownInterface" }
  | { readonly tag: "Failed" };
export type RemoteControlGroupOutcome =
  | { readonly tag: "Applied" }
  | { readonly tag: "UnknownInterface" }
  | { readonly tag: "Failed" };
export type RemoteControlDiscoveryGroupsInventoryOutcome =
  | { readonly tag: "Groups"; readonly data: { readonly value: RemoteControlDiscoveryGroups } }
  | { readonly tag: "UnknownInterface" }
  | { readonly tag: "Unsupported" };
export type RemoteControlDiscoveryGroupsReplaceOutcome =
  | { readonly tag: "Applied" }
  | { readonly tag: "Unchanged" }
  | { readonly tag: "UnknownInterface" }
  | { readonly tag: "Unsupported" };
export type RemoteControlInterfacePeersOutcome =
  | { readonly tag: "Page"; readonly data: { readonly value: RemoteControlInterfacePeerPage } }
  | { readonly tag: "UnknownInterface" };
export type RemoteControlInterfacePeerPage = { readonly id: RemoteControlInterfaceId; readonly peers: ReadonlyArray<RemoteControlInterfacePeer>; readonly continuation: RemoteControlPeerContinuation };
export type RemoteControlInterfacePeer = { readonly id: RemoteControlInterfaceId; readonly connection: RemoteControlConnectionState; readonly txBytes: bigint; readonly rxBytes: bigint; readonly links: number; readonly destinations: number; readonly rateBytesPerSec: number; readonly radio: RemoteControlRadioIndication; readonly details: RemoteControlPeerDetails };
export type RemoteControlRadioIndication =
  | { readonly tag: "NotRadio" }
  | { readonly tag: "Bluetooth"; readonly data: { readonly value: RemoteControlBluetoothIndication } }
  | { readonly tag: "Wifi"; readonly data: { readonly value: RemoteControlWifiIndication } }
  | { readonly tag: "LoRa"; readonly data: { readonly value: RemoteControlLoRaIndication } };
export type RemoteControlBluetoothIndication =
  | { readonly tag: "Pending" }
  | { readonly tag: "Rssi"; readonly data: { readonly value: RemoteControlRssiDbm } };
export type RemoteControlRssiDbm = { readonly value: number };
export type RemoteControlWifiIndication =
  | { readonly tag: "Unavailable" }
  | { readonly tag: "Pending" }
  | { readonly tag: "Rssi"; readonly data: { readonly value: RemoteControlRssiDbm } };
export type RemoteControlLoRaIndication =
  | { readonly tag: "Pending" }
  | { readonly tag: "Sample"; readonly data: { readonly rssi: RemoteControlRssiDbm; readonly snr?: RemoteControlSnrQuarterDb; readonly quality?: RemoteControlSignalQualityTenthsPercent } };
export type RemoteControlSnrQuarterDb = { readonly value: number };
export type RemoteControlSignalQualityTenthsPercent = { readonly value: number };
export type RemoteControlPeerDetails =
  | { readonly tag: "NotApplicable" }
  | { readonly tag: "Unknown" }
  | { readonly tag: "BleGatt" }
  | { readonly tag: "BleCoc" }
  | { readonly tag: "WifiRfChannel"; readonly data: { readonly value: number } };
export type RemoteControlPeerContinuation =
  | { readonly tag: "Complete" }
  | { readonly tag: "More"; readonly data: { readonly value: RemoteControlPeerCursor } };
export type RemoteControlInterfaceConfigOutcome =
  | { readonly tag: "Card"; readonly data: { readonly value: RemoteControlInterfaceCard } }
  | { readonly tag: "UnknownInterface" };
export type RemoteControlInterfaceCard = { readonly name: string; readonly group: string; readonly config: string; readonly failure: string; readonly destinations: number; readonly transportedLinks: number };
export type RemoteControlLoRaOutcome =
  | { readonly tag: "Applied" }
  | { readonly tag: "UnknownInterface" }
  | { readonly tag: "Failed" };
export type RemoteControlWifiStationOutcome =
  | { readonly tag: "Applied" }
  | { readonly tag: "UnknownInterface" }
  | { readonly tag: "Failed" };
export type RemoteControlControllerInventory = { readonly hashes: ReadonlyArray<RemoteControlIdentityHash>; readonly continuation: RemoteControlControllerContinuation };
export type RemoteControlControllerContinuation =
  | { readonly tag: "Complete" }
  | { readonly tag: "More"; readonly data: { readonly value: RemoteControlControllerCursor } };
export type RemoteControlAuthorizeControllerOutcome =
  | { readonly tag: "Applied" }
  | { readonly tag: "CapacityExhausted" }
  | { readonly tag: "Failed" }
  | { readonly tag: "Forbidden" }
  | { readonly tag: "Busy" };
export type RemoteControlRevokeControllerOutcome =
  | { readonly tag: "Applied" }
  | { readonly tag: "NotFound" }
  | { readonly tag: "Forbidden" }
  | { readonly tag: "Failed" }
  | { readonly tag: "Busy" };
export type RemoteControlBuildVersion = { readonly value: string };
export type RemoteControlPowerSnapshot = { readonly battery?: RemoteControlBatteryPercent; readonly externalPower: RemoteControlExternalPowerState };
export type RemoteControlBatteryPercent = { readonly value: number };
export type RemoteControlExternalPowerState =
  | { readonly tag: "Absent" }
  | { readonly tag: "Present"; readonly data: { readonly charging: RemoteControlChargingState } }
  | { readonly tag: "Unknown" };
export type RemoteControlChargingState =
  | { readonly tag: "Charging" }
  | { readonly tag: "Idle" }
  | { readonly tag: "Unknown" };
export type RemoteControlSleepOutcome =
  | { readonly tag: "Applied" }
  | { readonly tag: "Unavailable" }
  | { readonly tag: "Failed" };
export type RemoteControlApplyOutcome =
  | { readonly tag: "Applied" }
  | { readonly tag: "Unchanged" }
  | { readonly tag: "Scheduled" };
export type RemoteControlWifiStageOutcome =
  | { readonly tag: "Staged"; readonly data: { readonly value: RemoteControlWifiCredentialRevision } }
  | { readonly tag: "InvalidCredentials" };
export type RemoteControlWifiTransactionStatus =
  | { readonly tag: "FactoryProvisioning" }
  | { readonly tag: "Confirmed"; readonly data: { readonly revision: RemoteControlWifiCredentialRevision } }
  | { readonly tag: "Staged"; readonly data: { readonly revision: RemoteControlWifiCredentialRevision } }
  | { readonly tag: "AwaitingConfirmation"; readonly data: { readonly revision: RemoteControlWifiCredentialRevision; readonly remaining: RemoteControlWifiConfirmationRemaining } }
  | { readonly tag: "RollingBack"; readonly data: { readonly rejectedRevision: RemoteControlWifiCredentialRevision } };
export type RemoteControlWifiConfirmationRemaining = { readonly value: number };
export type RemoteControlProtocolError =
  | { readonly tag: "MalformedRequest" }
  | { readonly tag: "UnsupportedVersion"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownRequestKind"; readonly data: { readonly found: number } }
  | { readonly tag: "UnsupportedRequest"; readonly data: { readonly request: RemoteControlRequestKind } }
  | { readonly tag: "Busy"; readonly data: { readonly request: RemoteControlRequestKind } }
  | { readonly tag: "ApplyFailed"; readonly data: { readonly request: RemoteControlRequestKind } }
  | { readonly tag: "PersistenceFailed"; readonly data: { readonly request: RemoteControlRequestKind } }
  | { readonly tag: "RollbackFailed"; readonly data: { readonly request: RemoteControlRequestKind } }
  | { readonly tag: "InternalFailure"; readonly data: { readonly request: RemoteControlRequestKind } };
export type RemoteControlExchangeSettlement =
  | { readonly tag: "Completed"; readonly data: { readonly response: RemoteControlResponse; readonly rttMillis: bigint } }
  | { readonly tag: "Failed"; readonly data: { readonly failure: RemoteControlNativeRemoteControlError } };
export type RemoteControlNativeRemoteControlError =
  | { readonly tag: "Admission"; readonly data: { readonly value: RemoteControlNativeSubmitError } }
  | { readonly tag: "Connect"; readonly data: { readonly value: RemoteControlConnectRemoteControlTargetError } }
  | { readonly tag: "Exchange"; readonly data: { readonly value: RemoteControlError } }
  | { readonly tag: "TargetOperation"; readonly data: { readonly value: RemoteControlTargetOperationError } };
export type RemoteControlNativeSubmitError =
  | { readonly tag: "Busy" }
  | { readonly tag: "Stopped" };
export type RemoteControlConnectRemoteControlTargetError =
  | { readonly tag: "Resolve"; readonly data: { readonly value: RemoteControlResolveRemoteControlTargetControlError } }
  | { readonly tag: "EstablishLink"; readonly data: { readonly value: RemoteControlSendErrorRemoteControlEstablishLinkFailure } }
  | { readonly tag: "Identify"; readonly data: { readonly value: RemoteControlSendErrorRemoteControlIdentifyFailure } };
export type RemoteControlResolveRemoteControlTargetControlError =
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Unavailable" }
  | { readonly tag: "TargetNotAuthorized" };
export type RemoteControlSendErrorRemoteControlEstablishLinkFailure =
  | { readonly tag: "PayloadTooLarge" }
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Failed"; readonly data: { readonly value: RemoteControlEstablishLinkFailure } };
export type RemoteControlEstablishLinkFailure =
  | { readonly tag: "Rejected"; readonly data: { readonly value: RemoteControlEstablishLinkRejection } }
  | { readonly tag: "WriteFailed"; readonly data: { readonly value: RemoteControlWriteEstablishLinkRejection } }
  | { readonly tag: "Timeout" };
export type RemoteControlEstablishLinkRejection =
  | { readonly tag: "NoRouteToDestination" }
  | { readonly tag: "NotDirectlyReachable" };
export type RemoteControlWriteEstablishLinkRejection =
  | { readonly tag: "RouteVanished" }
  | { readonly tag: "Serialize" }
  | { readonly tag: "LinkTableFull" }
  | { readonly tag: "DuplicateLinkId" };
export type RemoteControlSendErrorRemoteControlIdentifyFailure =
  | { readonly tag: "PayloadTooLarge" }
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Failed"; readonly data: { readonly value: RemoteControlIdentifyFailure } };
export type RemoteControlIdentifyFailure =
  | { readonly tag: "Rejected"; readonly data: { readonly value: RemoteControlIdentifyRejection } }
  | { readonly tag: "WriteFailed" };
export type RemoteControlIdentifyRejection =
  | { readonly tag: "NoSuchLink" }
  | { readonly tag: "LinkNotActive" }
  | { readonly tag: "NotInitiator" }
  | { readonly tag: "IdentityNotHeld" };
export type RemoteControlError =
  | { readonly tag: "UnsupportedRequestKind"; readonly data: { readonly value: RemoteControlRequestKind } }
  | { readonly tag: "Encode"; readonly data: { readonly value: RemoteControlMessageWriteError } }
  | { readonly tag: "Request"; readonly data: { readonly value: RemoteControlSendErrorRemoteControlSendRequestFailure } }
  | { readonly tag: "Response"; readonly data: { readonly value: RemoteControlResponseParseError } }
  | { readonly tag: "Remote"; readonly data: { readonly value: RemoteControlProtocolError } }
  | { readonly tag: "UnexpectedResponse"; readonly data: { readonly expected: RemoteControlResponseKind; readonly found: RemoteControlResponseKind } }
  | { readonly tag: "AnnounceSelf"; readonly data: { readonly value: RemoteControlAnnounceSelfFailure } };
export type RemoteControlMessageWriteError =
  | { readonly tag: "BufferTooShort" }
  | { readonly tag: "InvalidRequestSet" };
export type RemoteControlSendErrorRemoteControlSendRequestFailure =
  | { readonly tag: "PayloadTooLarge" }
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Failed"; readonly data: { readonly value: RemoteControlSendRequestFailure } };
export type RemoteControlSendRequestFailure =
  | { readonly tag: "Rejected"; readonly data: { readonly value: RemoteControlSendRequestRejection } }
  | { readonly tag: "WriteFailed" }
  | { readonly tag: "Culled" }
  | { readonly tag: "Timeout" }
  | { readonly tag: "LinkClosed" }
  | { readonly tag: "ResponseTooLarge" }
  | { readonly tag: "ResponseTransferFailed"; readonly data: { readonly value: RemoteControlResourceFailureCause } }
  | { readonly tag: "ResourceCapacity" };
export type RemoteControlSendRequestRejection =
  | { readonly tag: "NoSuchLink" }
  | { readonly tag: "LinkNotActive" };
export type RemoteControlResourceFailureCause =
  | { readonly tag: "CancelledBySender" }
  | { readonly tag: "RefusedHashmapUpdate"; readonly data: { readonly value: RemoteControlApplyHashmapUpdateError } }
  | { readonly tag: "RetriesExhausted" }
  | { readonly tag: "LinkVanished" }
  | { readonly tag: "TransferUnopenable" }
  | { readonly tag: "TransferCorrupt" }
  | { readonly tag: "ProofUnsendable" }
  | { readonly tag: "DecompressionFailed" }
  | { readonly tag: "DecompressionTimedOut" }
  | { readonly tag: "OpenTimedOut" }
  | { readonly tag: "MetadataOverrun" };
export type RemoteControlApplyHashmapUpdateError =
  | { readonly tag: "BeyondPartCount" }
  | { readonly tag: "SkipsAhead" }
  | { readonly tag: "HashmapTooLong" }
  | { readonly tag: "HashmapRagged" };
export type RemoteControlResponseParseError =
  | { readonly tag: "Truncated" }
  | { readonly tag: "UnsupportedVersion"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownResponseKind"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownAnnounceSelfOutcome"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownPowerOutcome"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownModeOutcome"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownGroupOutcome"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownLoRaOutcome"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownWifiStationOutcome"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownAuthorizeControllerOutcome"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownRevokeControllerOutcome"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownSleepOutcome"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownApplyOutcome"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownWifiStageOutcome"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownWifiTransactionStatus"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownProtocolErrorKind"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownRequestKind"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownInterfaceKind"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownInterfaceMode"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownConnectionState"; readonly data: { readonly found: number } }
  | { readonly tag: "NonCanonicalRequestSet" }
  | { readonly tag: "NonCanonicalCursor" }
  | { readonly tag: "Malformed" };
export type RemoteControlResponseKind =
  | { readonly tag: "Describe" }
  | { readonly tag: "AnnounceSelf" }
  | { readonly tag: "InventoryInterfaces" }
  | { readonly tag: "SetInterfacePower" }
  | { readonly tag: "SleepRadios" }
  | { readonly tag: "WakeRadios" }
  | { readonly tag: "SetInterfaceMode" }
  | { readonly tag: "SetInterfaceGroup" }
  | { readonly tag: "InventoryInterfacePeers" }
  | { readonly tag: "InventoryInterfaceConfig" }
  | { readonly tag: "SetInterfaceLoRaProfile" }
  | { readonly tag: "DescribeBuild" }
  | { readonly tag: "SetInterfaceWifiStation" }
  | { readonly tag: "InventoryControllers" }
  | { readonly tag: "AuthorizeController" }
  | { readonly tag: "RevokeController" }
  | { readonly tag: "DescribePower" }
  | { readonly tag: "SetSystemPower" }
  | { readonly tag: "SetGnssPower" }
  | { readonly tag: "SetDisplayVisibility" }
  | { readonly tag: "SetDisplayAutoOff" }
  | { readonly tag: "SetStationUplink" }
  | { readonly tag: "SetEspRadioMode" }
  | { readonly tag: "StageWifiCredentials" }
  | { readonly tag: "ActivateWifiCredentials" }
  | { readonly tag: "ConfirmWifiCredentials" }
  | { readonly tag: "CancelWifiCredentials" }
  | { readonly tag: "InspectWifiTransaction" }
  | { readonly tag: "InventoryInterfaceDiscoveryGroups" }
  | { readonly tag: "ReplaceInterfaceDiscoveryGroups" }
  | { readonly tag: "ProtocolError" };
export type RemoteControlAnnounceSelfFailure =
  | { readonly tag: "Unavailable" }
  | { readonly tag: "Rejected" }
  | { readonly tag: "WriteFailed" };
export type RemoteControlTargetOperationError =
  | { readonly tag: "NotPermitted"; readonly data: { readonly value: RemoteControlRequestKind } }
  | { readonly tag: "Exchange"; readonly data: { readonly value: RemoteControlError } };
export type RemoteControlNativeRemoteControlConfig = { readonly controllerIdentity: IdentityConfig; readonly targetIdentity: IdentityConfig; readonly initialControllerGrants: ReadonlyArray<RemoteControlControllerGrant>; readonly selfAnnouncement: RemoteControlSelfAnnouncement; readonly capabilities: RemoteControlCapabilities };
export type RemoteControlControllerGrant = { readonly controller: RemoteControlControllerIdentity; readonly authority: RemoteControlControllerAuthority; readonly permittedRequests: RemoteControlRequestSet };
export type RemoteControlControllerAuthority =
  | { readonly tag: "Operator" }
  | { readonly tag: "Administrator" };
export type RemoteControlSelfAnnouncement =
  | { readonly tag: "Unavailable" }
  | { readonly tag: "Destination"; readonly data: { readonly value: RemoteControlDestinationHash } };
export type RemoteControlDestinationHash = { readonly value: Uint8Array };
export type RemoteControlCapabilities = { readonly requests: RemoteControlRequestSet };
export type RemoteControlNativeRemoteControlEvent =
  | { readonly tag: "PairingAvailable"; readonly data: { readonly endpoint: RemoteControlPairingEndpoint; readonly observedAt: RemoteControlInstantMillis; readonly expiresAt: RemoteControlInstantMillis; readonly hops: number; readonly sourceInterface: RemoteControlInterfaceId; readonly publicAppData: Uint8Array } }
  | { readonly tag: "TargetConfirmationRequired"; readonly data: { readonly confirmation: RemoteControlNativeRemoteControlConfirmation; readonly window: RemoteControlTargetPairingAttemptWindow } }
  | { readonly tag: "TargetControllerCommitted"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "TargetAuthorizationRequired"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId; readonly grant: RemoteControlControllerGrant } }
  | { readonly tag: "TargetAuthorizationPersisted"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "TargetExpiredDuringAuthorization"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "ControllerConfirmationRequired"; readonly data: { readonly confirmation: RemoteControlNativeRemoteControlConfirmation; readonly window: RemoteControlControllerPairingAttemptWindow } }
  | { readonly tag: "ControllerPersistenceRequired"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId; readonly context: RemoteControlPairingContext; readonly target: RemoteControlTargetIdentity; readonly authority: RemoteControlControllerAuthority; readonly permittedRequests: RemoteControlRequestSet } }
  | { readonly tag: "ControllerAuthorizationPersisted"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "ControllerAuthorizationPersistenceFailed"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "ControllerExpired"; readonly data: { readonly aborted: RemoteControlControllerPairingAborted } }
  | { readonly tag: "ControllerLinkClosed"; readonly data: { readonly aborted: RemoteControlControllerPairingAborted } }
  | { readonly tag: "TargetExpired"; readonly data: { readonly aborted: RemoteControlTargetPairingAborted } }
  | { readonly tag: "TargetLinkClosed"; readonly data: { readonly aborted: RemoteControlTargetPairingAborted } }
  | { readonly tag: "TargetCompletionRetentionExpired"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "TargetCompletionLinkClosed"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } };
export type RemoteControlPairingEndpoint = { readonly destinationHash: RemoteControlDestinationHash };
export type RemoteControlInstantMillis = { readonly value: bigint };
export type RemoteControlNativeRemoteControlConfirmation = { readonly attemptId: RemoteControlPairingAttemptId; readonly context: RemoteControlPairingContext; readonly controller: RemoteControlControllerIdentity; readonly target: RemoteControlTargetIdentity; readonly permissions: RemoteControlPairingPermissions; readonly confirmationCode: string };
export type RemoteControlPairingAttemptId = { readonly value: Uint8Array };
export type RemoteControlPairingContext = { readonly endpoint: RemoteControlPairingEndpoint; readonly linkId: RemoteControlLinkId };
export type RemoteControlLinkId = { readonly value: Uint8Array };
export type RemoteControlTargetIdentity = { readonly publicKeys: Uint8Array };
export type RemoteControlPairingPermissions = { readonly authority: RemoteControlControllerAuthority; readonly permittedRequests: RemoteControlRequestSet };
export type RemoteControlTargetPairingAttemptWindow = { readonly startedAt: RemoteControlInstantMillis; readonly attemptTimeout: RemoteControlPairingAttemptTimeout; readonly expiresAt: RemoteControlInstantMillis };
export type RemoteControlPairingAttemptTimeout = { readonly value: bigint };
export type RemoteControlControllerPairingAttemptWindow = { readonly offeredAt: RemoteControlInstantMillis; readonly attemptTimeout: RemoteControlPairingAttemptTimeout; readonly expiresAt: RemoteControlInstantMillis };
export type RemoteControlControllerPairingAborted =
  | { readonly tag: "AwaitingOffer"; readonly data: { readonly context: RemoteControlPairingContext } }
  | { readonly tag: "AwaitingApproval"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId; readonly context: RemoteControlPairingContext } }
  | { readonly tag: "AwaitingCompletion"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId; readonly context: RemoteControlPairingContext } };
export type RemoteControlTargetPairingAborted =
  | { readonly tag: "OfferPrepared"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId; readonly context: RemoteControlPairingContext } }
  | { readonly tag: "AwaitingBoth"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId; readonly context: RemoteControlPairingContext } }
  | { readonly tag: "AwaitingTargetApproval"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId; readonly context: RemoteControlPairingContext; readonly responder: RemoteControlTargetPairingResponder } }
  | { readonly tag: "AwaitingControllerCommit"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId; readonly context: RemoteControlPairingContext } };
export type RemoteControlTargetPairingResponder = { readonly linkId: RemoteControlLinkId; readonly requestId: RemoteControlRequestId };
export type RemoteControlRequestId = { readonly value: Uint8Array };
export type RemoteControlBeginRemoteControlControllerPairing = { readonly context: RemoteControlPairingContext; readonly invitationCode: RemoteControlPairingInvitationCode; readonly pairingExpiresAt: RemoteControlInstantMillis };
export type RemoteControlPairingInvitationCode = { readonly value: number };
export type RemoteControlControllerPairingResponseReceived = { readonly delivered: RemoteControlPacketReceiptDelivered; readonly admission: RemoteControlAdmitRemoteControlControllerPairingResponseOutcome; readonly effect: RemoteControlControllerPairingResponseEffect };
export type RemoteControlPacketReceiptDelivered = { readonly rtt: RemoteControlRttMillis; readonly evidence: RemoteControlDeliveryEvidence };
export type RemoteControlRttMillis = { readonly value: bigint };
export type RemoteControlDeliveryEvidence =
  | { readonly tag: "Proof"; readonly data: { readonly value: RemoteControlDeliveryProof } }
  | { readonly tag: "Response" };
export type RemoteControlDeliveryProof =
  | { readonly tag: "Explicit"; readonly data: { readonly value: RemoteControlPacketHash } }
  | { readonly tag: "Implicit"; readonly data: { readonly value: RemoteControlPacketHash } };
export type RemoteControlPacketHash = { readonly value: Uint8Array };
export type RemoteControlAdmitRemoteControlControllerPairingResponseOutcome =
  | { readonly tag: "NoActivePairing" }
  | { readonly tag: "UnrelatedLink"; readonly data: { readonly expected: RemoteControlLinkId; readonly received: RemoteControlLinkId } }
  | { readonly tag: "MalformedEnvelope"; readonly data: { readonly value: RemoteControlPackedBinaryParseError } }
  | { readonly tag: "MalformedResponse"; readonly data: { readonly value: RemoteControlPairingMessageParseError } }
  | { readonly tag: "Offer"; readonly data: { readonly value: RemoteControlReceiveRemoteControlControllerPairingOfferOutcome } }
  | { readonly tag: "Completed"; readonly data: { readonly value: RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome } }
  | { readonly tag: "InvariantViolation"; readonly data: { readonly value: RemoteControlControllerPairingResponseBridgeInvariantViolation } };
export type RemoteControlPackedBinaryParseError =
  | { readonly tag: "Truncated" }
  | { readonly tag: "NotBinary" }
  | { readonly tag: "LengthOutOfRange"; readonly data: { readonly declared: number } }
  | { readonly tag: "LengthMismatch"; readonly data: { readonly declared: bigint; readonly actual: bigint } };
export type RemoteControlPairingMessageParseError =
  | { readonly tag: "TooLong"; readonly data: { readonly actual: bigint; readonly maximum: bigint } }
  | { readonly tag: "Truncated" }
  | { readonly tag: "UnsupportedVersion"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownKind"; readonly data: { readonly found: number } }
  | { readonly tag: "UnexpectedKind"; readonly data: { readonly direction: RemoteControlPairingMessageDirection; readonly found: RemoteControlPairingMessageKind } }
  | { readonly tag: "InvalidSigningPublicKey"; readonly data: { readonly role: RemoteControlPairingIdentityRole } }
  | { readonly tag: "UnknownRequestKind"; readonly data: { readonly found: number } }
  | { readonly tag: "UnknownAuthority"; readonly data: { readonly found: number } }
  | { readonly tag: "TooManyPermissionsForVersion"; readonly data: { readonly version: RemoteControlPairingProtocolVersion; readonly actual: bigint; readonly maximum: bigint } }
  | { readonly tag: "RequestUnsupportedForVersion"; readonly data: { readonly version: RemoteControlPairingProtocolVersion; readonly request: RemoteControlRequestKind } }
  | { readonly tag: "NonCanonicalPermissions" }
  | { readonly tag: "InvalidPermissions"; readonly data: { readonly value: RemoteControlPairingPermissionsError } }
  | { readonly tag: "InvalidAttemptTimeout"; readonly data: { readonly value: RemoteControlPairingAttemptTimeoutError } }
  | { readonly tag: "TrailingBytes"; readonly data: { readonly actual: bigint } };
export type RemoteControlPairingMessageDirection =
  | { readonly tag: "Request" }
  | { readonly tag: "Response" };
export type RemoteControlPairingMessageKind =
  | { readonly tag: "Begin" }
  | { readonly tag: "Offer" }
  | { readonly tag: "Commit" }
  | { readonly tag: "Completed" };
export type RemoteControlPairingIdentityRole =
  | { readonly tag: "Controller" }
  | { readonly tag: "Target" };
export type RemoteControlPairingProtocolVersion =
  | { readonly tag: "V2" }
  | { readonly tag: "V3" }
  | { readonly tag: "V4" };
export type RemoteControlPairingPermissionsError =
  | { readonly tag: "NoPermittedRequests" }
  | { readonly tag: "AdministratorRequestRequiresAuthority"; readonly data: { readonly request: RemoteControlRequestKind } };
export type RemoteControlPairingAttemptTimeoutError =
  | { readonly tag: "Zero" }
  | { readonly tag: "TooLong"; readonly data: { readonly actual: RemoteControlDurationMillis; readonly maximum: RemoteControlDurationMillis } };
export type RemoteControlDurationMillis = { readonly value: bigint };
export type RemoteControlReceiveRemoteControlControllerPairingOfferOutcome =
  | { readonly tag: "ConfirmationRequired"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "Expired"; readonly data: { readonly expired: RemoteControlControllerPairingAborted } }
  | { readonly tag: "Rejected"; readonly data: { readonly reason: RemoteControlPairingOfferVerificationError } }
  | { readonly tag: "NoActiveAttempt" }
  | { readonly tag: "Unexpected"; readonly data: { readonly active: RemoteControlControllerPairingActivity } }
  | { readonly tag: "PairingUnavailable"; readonly data: { readonly reason: RemoteControlControllerPairingAttemptWindowError } };
export type RemoteControlPairingOfferVerificationError =
  | { readonly tag: "ProtocolVersionMismatch"; readonly data: { readonly begin: RemoteControlPairingProtocolVersion; readonly offer: RemoteControlPairingProtocolVersion } }
  | { readonly tag: "InvalidTargetSignature" };
export type RemoteControlControllerPairingActivity =
  | { readonly tag: "AwaitingOffer" }
  | { readonly tag: "AwaitingApproval"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "AwaitingCompletion"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "Persisting"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } };
export type RemoteControlControllerPairingAttemptWindowError =
  | { readonly tag: "PairingWindowElapsed"; readonly data: { readonly offeredAt: RemoteControlInstantMillis; readonly pairingExpiresAt: RemoteControlInstantMillis } };
export type RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome =
  | { readonly tag: "PersistenceOwed"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "Expired"; readonly data: { readonly expired: RemoteControlControllerPairingAborted } }
  | { readonly tag: "Rejected"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId; readonly reason: RemoteControlPairingCompletedVerificationError } }
  | { readonly tag: "NoActiveAttempt" }
  | { readonly tag: "OfferNotReceived" }
  | { readonly tag: "ApprovalRequired"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "AlreadyReceived"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } };
export type RemoteControlPairingCompletedVerificationError =
  | { readonly tag: "ProtocolVersionMismatch"; readonly data: { readonly expected: RemoteControlPairingProtocolVersion; readonly found: RemoteControlPairingProtocolVersion } }
  | { readonly tag: "TranscriptMismatch"; readonly data: { readonly expected: RemoteControlPairingTranscriptDigest; readonly found: RemoteControlPairingTranscriptDigest } }
  | { readonly tag: "InvalidTargetSignature" };
export type RemoteControlPairingTranscriptDigest = { readonly value: Uint8Array };
export type RemoteControlControllerPairingResponseBridgeInvariantViolation =
  | { readonly tag: "ConfirmationStateUnavailable"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "PersistenceStateUnavailable"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } };
export type RemoteControlControllerPairingResponseEffect =
  | { readonly tag: "Advanced" }
  | { readonly tag: "Expired"; readonly data: { readonly retiredLink: RemoteControlLinkId } }
  | { readonly tag: "NotAdvanced"; readonly data: { readonly value: RemoteControlFailRemoteControlControllerPairingRequestOutcome } };
export type RemoteControlFailRemoteControlControllerPairingRequestOutcome =
  | { readonly tag: "Aborted"; readonly data: { readonly aborted: RemoteControlControllerPairingAborted } }
  | { readonly tag: "UnrelatedLink" }
  | { readonly tag: "NoActiveAttempt" }
  | { readonly tag: "PersistenceInProgress"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } };
export type RemoteControlPairingControlErrorRemoteControlBeginRemoteControlControllerPairingControlFailure =
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Failed"; readonly data: { readonly value: RemoteControlBeginRemoteControlControllerPairingControlFailure } };
export type RemoteControlBeginRemoteControlControllerPairingControlFailure =
  | { readonly tag: "Begin"; readonly data: { readonly value: RemoteControlBeginRemoteControlControllerPairingFailure } }
  | { readonly tag: "Identify"; readonly data: { readonly failure: RemoteControlSendErrorRemoteControlIdentifyFailure; readonly cleanup: RemoteControlPairingLinkCleanupOutcome } }
  | { readonly tag: "Request"; readonly data: { readonly value: RemoteControlControllerPairingRequestFailure } };
export type RemoteControlBeginRemoteControlControllerPairingFailure =
  | { readonly tag: "ControllerIdentityUnavailable" }
  | { readonly tag: "Busy"; readonly data: { readonly active: RemoteControlControllerPairingActivity } }
  | { readonly tag: "PairingUnavailable"; readonly data: { readonly reason: RemoteControlControllerPairingWindowError } }
  | { readonly tag: "RequestBuild"; readonly data: { readonly failure: RemoteControlControllerPairingRequestBuildError; readonly rollback: RemoteControlFailRemoteControlControllerPairingRequestOutcome } };
export type RemoteControlControllerPairingWindowError =
  | { readonly tag: "DeadlineNotFuture"; readonly data: { readonly startedAt: RemoteControlInstantMillis; readonly expiresAt: RemoteControlInstantMillis } };
export type RemoteControlControllerPairingRequestBuildError =
  | { readonly tag: "Encode"; readonly data: { readonly value: RemoteControlPairingMessageWriteError } }
  | { readonly tag: "Pack"; readonly data: { readonly value: RemoteControlPackBinaryError } }
  | { readonly tag: "Capacity"; readonly data: { readonly required: bigint; readonly maximum: bigint } };
export type RemoteControlPairingMessageWriteError =
  | { readonly tag: "BufferTooShort"; readonly data: { readonly required: bigint; readonly actual: bigint } };
export type RemoteControlPackBinaryError =
  | { readonly tag: "BufferTooShort" }
  | { readonly tag: "LengthOutOfRange" };
export type RemoteControlPairingLinkCleanupOutcome =
  | { readonly tag: "Queued" }
  | { readonly tag: "NotQueued" };
export type RemoteControlControllerPairingRequestFailure = { readonly cause: RemoteControlControllerPairingRequestFailureCause; readonly exchange: RemoteControlFailRemoteControlControllerPairingRequestOutcome };
export type RemoteControlControllerPairingRequestFailureCause =
  | { readonly tag: "Request"; readonly data: { readonly value: RemoteControlSendRequestFailure } }
  | { readonly tag: "ResourceResponseUnsupported" };
export type RemoteControlBeginRemoteControlControllerPairingControlError =
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Failed"; readonly data: { readonly value: RemoteControlBeginRemoteControlControllerPairingControlFailure } };
export type RemoteControlBeginRemoteControlControllerPairingSettlement =
  | { readonly tag: "Completed"; readonly data: { readonly value: RemoteControlControllerPairingResponseReceived } }
  | { readonly tag: "Failed"; readonly data: { readonly failure: RemoteControlBeginRemoteControlControllerPairingControlError } };
export type RemoteControlApproveRemoteControlControllerPairing = { readonly attemptId: RemoteControlPairingAttemptId };
export type RemoteControlPairingControlErrorRemoteControlApproveRemoteControlControllerPairingControlFailure =
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Failed"; readonly data: { readonly value: RemoteControlApproveRemoteControlControllerPairingControlFailure } };
export type RemoteControlApproveRemoteControlControllerPairingControlFailure =
  | { readonly tag: "Approve"; readonly data: { readonly value: RemoteControlApproveRemoteControlControllerPairingFailure } }
  | { readonly tag: "Request"; readonly data: { readonly value: RemoteControlControllerPairingRequestFailure } };
export type RemoteControlApproveRemoteControlControllerPairingFailure =
  | { readonly tag: "Expired"; readonly data: { readonly expired: RemoteControlControllerPairingAborted; readonly retiredLink: RemoteControlLinkId } }
  | { readonly tag: "NoActiveAttempt" }
  | { readonly tag: "OfferNotReceived" }
  | { readonly tag: "AttemptMismatch"; readonly data: { readonly requested: RemoteControlPairingAttemptId; readonly active: RemoteControlPairingAttemptId } }
  | { readonly tag: "PersistenceInProgress"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "RequestBuild"; readonly data: { readonly failure: RemoteControlControllerPairingRequestBuildError; readonly rollback: RemoteControlFailRemoteControlControllerPairingRequestOutcome } };
export type RemoteControlApproveRemoteControlControllerPairingControlError =
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Failed"; readonly data: { readonly value: RemoteControlApproveRemoteControlControllerPairingControlFailure } };
export type RemoteControlApproveRemoteControlControllerPairingSettlement =
  | { readonly tag: "Completed"; readonly data: { readonly value: RemoteControlControllerPairingResponseReceived } }
  | { readonly tag: "Failed"; readonly data: { readonly failure: RemoteControlApproveRemoteControlControllerPairingControlError } };
export type RemoteControlRejectRemoteControlControllerPairing = { readonly attemptId: RemoteControlPairingAttemptId };
export type RemoteControlControllerPairingRejection = { readonly aborted: RemoteControlControllerPairingAborted; readonly retiredLink: RemoteControlLinkId };
export type RemoteControlPairingControlErrorRemoteControlRejectRemoteControlControllerPairingFailure =
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Failed"; readonly data: { readonly value: RemoteControlRejectRemoteControlControllerPairingFailure } };
export type RemoteControlRejectRemoteControlControllerPairingFailure =
  | { readonly tag: "Expired"; readonly data: { readonly expired: RemoteControlControllerPairingAborted; readonly retiredLink: RemoteControlLinkId } }
  | { readonly tag: "NoActiveAttempt" }
  | { readonly tag: "OfferNotReceived" }
  | { readonly tag: "AttemptMismatch"; readonly data: { readonly requested: RemoteControlPairingAttemptId; readonly active: RemoteControlPairingAttemptId } }
  | { readonly tag: "AlreadyApproved"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "PersistenceInProgress"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } };
export type RemoteControlRejectRemoteControlControllerPairingControlError =
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Failed"; readonly data: { readonly value: RemoteControlRejectRemoteControlControllerPairingFailure } };
export type RemoteControlRejectRemoteControlControllerPairingSettlement =
  | { readonly tag: "Completed"; readonly data: { readonly value: RemoteControlControllerPairingRejection } }
  | { readonly tag: "Failed"; readonly data: { readonly failure: RemoteControlRejectRemoteControlControllerPairingControlError } };
export type RemoteControlApproveRemoteControlTargetPairing = { readonly attemptId: RemoteControlPairingAttemptId };
export type RemoteControlTargetPairingApproval =
  | { readonly tag: "AwaitingControllerCommit"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "AuthorizationOwed"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId; readonly grant: RemoteControlControllerGrant } };
export type RemoteControlPairingControlErrorRemoteControlApproveRemoteControlTargetPairingFailure =
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Failed"; readonly data: { readonly value: RemoteControlApproveRemoteControlTargetPairingFailure } };
export type RemoteControlApproveRemoteControlTargetPairingFailure =
  | { readonly tag: "Expired"; readonly data: { readonly expired: RemoteControlTargetPairingAborted; readonly retiredLink: RemoteControlLinkId } }
  | { readonly tag: "NoActiveAttempt" }
  | { readonly tag: "AttemptMismatch"; readonly data: { readonly requested: RemoteControlPairingAttemptId; readonly active: RemoteControlPairingAttemptId } }
  | { readonly tag: "OfferPendingDispatch"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "AlreadyApproved"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "FinalizationInProgress"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "CompletionRetentionExpired"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId; readonly retiredLink: RemoteControlLinkId } };
export type RemoteControlApproveRemoteControlTargetPairingControlError =
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Failed"; readonly data: { readonly value: RemoteControlApproveRemoteControlTargetPairingFailure } };
export type RemoteControlApproveRemoteControlTargetPairingSettlement =
  | { readonly tag: "Completed"; readonly data: { readonly value: RemoteControlTargetPairingApproval } }
  | { readonly tag: "Failed"; readonly data: { readonly failure: RemoteControlApproveRemoteControlTargetPairingControlError } };
export type RemoteControlRejectRemoteControlTargetPairing = { readonly attemptId: RemoteControlPairingAttemptId };
export type RemoteControlTargetPairingRejection = { readonly aborted: RemoteControlTargetPairingAborted; readonly retiredLink: RemoteControlLinkId };
export type RemoteControlPairingControlErrorRemoteControlRejectRemoteControlTargetPairingFailure =
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Failed"; readonly data: { readonly value: RemoteControlRejectRemoteControlTargetPairingFailure } };
export type RemoteControlRejectRemoteControlTargetPairingFailure =
  | { readonly tag: "Expired"; readonly data: { readonly expired: RemoteControlTargetPairingAborted; readonly retiredLink: RemoteControlLinkId } }
  | { readonly tag: "NoActiveAttempt" }
  | { readonly tag: "AttemptMismatch"; readonly data: { readonly requested: RemoteControlPairingAttemptId; readonly active: RemoteControlPairingAttemptId } }
  | { readonly tag: "OfferPendingDispatch"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "AlreadyApproved"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "FinalizationInProgress"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId } }
  | { readonly tag: "CompletionRetentionExpired"; readonly data: { readonly attemptId: RemoteControlPairingAttemptId; readonly retiredLink: RemoteControlLinkId } };
export type RemoteControlRejectRemoteControlTargetPairingControlError =
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Failed"; readonly data: { readonly value: RemoteControlRejectRemoteControlTargetPairingFailure } };
export type RemoteControlRejectRemoteControlTargetPairingSettlement =
  | { readonly tag: "Completed"; readonly data: { readonly value: RemoteControlTargetPairingRejection } }
  | { readonly tag: "Failed"; readonly data: { readonly failure: RemoteControlRejectRemoteControlTargetPairingControlError } };
export type RemoteControlTargetInventory = { readonly targets: ReadonlyArray<RemoteControlIdentityHash> };
export type RemoteControlTargetInventoryControlError =
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Unavailable" }
  | { readonly tag: "Inventory"; readonly data: { readonly value: RemoteControlTargetInventoryError } };
export type RemoteControlTargetInventoryError =
  | { readonly tag: "CapacityInvariantViolation" };
export type RemoteControlTargetInventorySettlement =
  | { readonly tag: "Completed"; readonly data: { readonly value: RemoteControlTargetInventory } }
  | { readonly tag: "Failed"; readonly data: { readonly failure: RemoteControlTargetInventoryControlError } };
export type RemoteControlResolvedRemoteControlTarget = { readonly target: RemoteControlIdentityHash; readonly endpoint: RemoteControlEndpoint; readonly controller: RemoteControlControllerIdentity; readonly permittedRequests: RemoteControlRequestSet };
export type RemoteControlEndpoint = { readonly destinationHash: RemoteControlDestinationHash };
export type RemoteControlResolveRemoteControlTargetSettlement =
  | { readonly tag: "Completed"; readonly data: { readonly value: RemoteControlResolvedRemoteControlTarget } }
  | { readonly tag: "Failed"; readonly data: { readonly failure: RemoteControlResolveRemoteControlTargetControlError } };
export type RemoteControlTargetAccess = { readonly target: RemoteControlTargetIdentity; readonly authority: RemoteControlControllerAuthority; readonly permittedRequests: RemoteControlRequestSet };
export type RemoteControlSetRemoteControlTargetAccessOutcome =
  | { readonly tag: "Added" }
  | { readonly tag: "Unchanged" }
  | { readonly tag: "Updated"; readonly data: { readonly previous: RemoteControlTargetAccess } };
export type RemoteControlSetRemoteControlTargetAccessControlError =
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Unavailable" }
  | { readonly tag: "CapacityExhausted" };
export type RemoteControlSetRemoteControlTargetAccessSettlement =
  | { readonly tag: "Completed"; readonly data: { readonly value: RemoteControlSetRemoteControlTargetAccessOutcome } }
  | { readonly tag: "Failed"; readonly data: { readonly failure: RemoteControlSetRemoteControlTargetAccessControlError } };
export type RemoteControlForgetRemoteControlTargetOutcome =
  | { readonly tag: "Forgotten"; readonly data: { readonly access: RemoteControlTargetAccess } }
  | { readonly tag: "NotFound" };
export type RemoteControlForgetRemoteControlTargetControlError =
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Unavailable" };
export type RemoteControlForgetRemoteControlTargetSettlement =
  | { readonly tag: "Completed"; readonly data: { readonly value: RemoteControlForgetRemoteControlTargetOutcome } }
  | { readonly tag: "Failed"; readonly data: { readonly failure: RemoteControlForgetRemoteControlTargetControlError } };
export type RemoteControlOpenRemoteControlPairing = { readonly target: RemoteControlEgressTarget; readonly expiresAfter: RemoteControlPairingExpiresAfter; readonly attemptTimeout: RemoteControlPairingAttemptTimeout; readonly permissions: RemoteControlPairingPermissions; readonly publicAppData: RemoteControlPairingPublicAppDataBytes };
export type RemoteControlEgressTarget =
  | { readonly tag: "AllInterfaces" }
  | { readonly tag: "Interface"; readonly data: { readonly value: RemoteControlInterfaceId } };
export type RemoteControlPairingExpiresAfter = { readonly value: bigint };
export type RemoteControlPairingPublicAppDataBytes = { readonly value: Uint8Array };
export type RemoteControlPairingOpened = { readonly endpoint: RemoteControlPairingEndpoint; readonly expiresAt: RemoteControlInstantMillis; readonly invitationCode: RemoteControlPairingInvitationCode };
export type RemoteControlPairingControlErrorRemoteControlOpenRemoteControlPairingFailure =
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Failed"; readonly data: { readonly value: RemoteControlOpenRemoteControlPairingFailure } };
export type RemoteControlOpenRemoteControlPairingFailure =
  | { readonly tag: "Rejected"; readonly data: { readonly value: RemoteControlOpenRemoteControlPairingRejection } }
  | { readonly tag: "IdentityGenerationExhausted" }
  | { readonly tag: "HoldIdentity"; readonly data: { readonly value: RemoteControlHoldIdentityError } }
  | { readonly tag: "RegisterEndpoint"; readonly data: { readonly value: RemoteControlRegisterDestinationError } }
  | { readonly tag: "ConfigureRequestLimit" }
  | { readonly tag: "RegisterRequestEndpoint"; readonly data: { readonly value: RemoteControlTablePushError } }
  | { readonly tag: "WriteAvailability"; readonly data: { readonly value: RemoteControlPairingAvailabilityWriteError } }
  | { readonly tag: "PayloadCapacity" }
  | { readonly tag: "WritePacket"; readonly data: { readonly value: RemoteControlSendPlainPacketWriteError } };
export type RemoteControlOpenRemoteControlPairingRejection =
  | { readonly tag: "Unavailable" }
  | { readonly tag: "AlreadyOpen" }
  | { readonly tag: "NoTransmittingInterfaces" }
  | { readonly tag: "EgressTarget"; readonly data: { readonly value: RemoteControlEgressTargetRejection } }
  | { readonly tag: "AttemptTimeoutExceedsWindow"; readonly data: { readonly attemptTimeout: RemoteControlPairingAttemptTimeout; readonly pairingExpiresAfter: RemoteControlPairingExpiresAfter } }
  | { readonly tag: "DeadlineOverflow" };
export type RemoteControlEgressTargetRejection =
  | { readonly tag: "UnknownInterface" }
  | { readonly tag: "InterfaceCannotTransmit" };
export type RemoteControlHoldIdentityError =
  | { readonly tag: "StoreFull" };
export type RemoteControlRegisterDestinationError =
  | { readonly tag: "Name"; readonly data: { readonly value: RemoteControlExpandNameError } }
  | { readonly tag: "RegistryFull" }
  | { readonly tag: "UnknownIdentity" }
  | { readonly tag: "RatchetTableFull" }
  | { readonly tag: "AppDataTooLong" }
  | { readonly tag: "InvalidGroupKey" };
export type RemoteControlExpandNameError =
  | { readonly tag: "DotInComponent" }
  | { readonly tag: "NameTooLong" };
export type RemoteControlTablePushError =
  | { readonly tag: "TableFull" };
export type RemoteControlPairingAvailabilityWriteError =
  | { readonly tag: "BufferTooShort"; readonly data: { readonly required: bigint; readonly actual: bigint } }
  | { readonly tag: "BuildAnnounce"; readonly data: { readonly value: RemoteControlAnnounceBuildError } }
  | { readonly tag: "WriteAnnounce"; readonly data: { readonly value: RemoteControlWireError } };
export type RemoteControlAnnounceBuildError =
  | { readonly tag: "AnnounceTooLarge" };
export type RemoteControlWireError =
  | { readonly tag: "BufferTooShort" };
export type RemoteControlSendPlainPacketWriteError =
  | { readonly tag: "Serialize" };
export type RemoteControlOpenRemoteControlPairingControlError =
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Failed"; readonly data: { readonly value: RemoteControlOpenRemoteControlPairingFailure } };
export type RemoteControlOpenRemoteControlPairingSettlement =
  | { readonly tag: "Completed"; readonly data: { readonly value: RemoteControlPairingOpened } }
  | { readonly tag: "Failed"; readonly data: { readonly failure: RemoteControlOpenRemoteControlPairingControlError } };
export type RemoteControlCloseRemoteControlPairingOutcome =
  | { readonly tag: "Closed"; readonly data: { readonly endpoint: RemoteControlPairingEndpoint } }
  | { readonly tag: "AlreadyClosed" };
export type RemoteControlPairingControlErrorRemoteControlCloseRemoteControlPairingFailure =
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Failed"; readonly data: { readonly value: RemoteControlCloseRemoteControlPairingFailure } };
export type RemoteControlCloseRemoteControlPairingFailure =
  | { readonly tag: "Unavailable" }
  | { readonly tag: "RetirementIncomplete"; readonly data: { readonly firstRemainingLink: RemoteControlLinkId; readonly retiredLinks: RemoteControlLinkCount } }
  | { readonly tag: "EndpointNotRegistered" }
  | { readonly tag: "IdentityNotHeld" };
export type RemoteControlLinkCount = { readonly value: bigint };
export type RemoteControlCloseRemoteControlPairingControlError =
  | { readonly tag: "NodeStopped" }
  | { readonly tag: "Busy" }
  | { readonly tag: "Failed"; readonly data: { readonly value: RemoteControlCloseRemoteControlPairingFailure } };
export type RemoteControlCloseRemoteControlPairingSettlement =
  | { readonly tag: "Completed"; readonly data: { readonly value: RemoteControlCloseRemoteControlPairingOutcome } }
  | { readonly tag: "Failed"; readonly data: { readonly failure: RemoteControlCloseRemoteControlPairingControlError } };
