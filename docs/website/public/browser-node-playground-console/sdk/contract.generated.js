export const HOST_CONTRACT_ABI = 1;
export const HOST_SCHEMA_VERSION = 1;
export const PRODUCT_VERSION = "0.3.6";
export const DESTINATION_HASH_LENGTH = 16;
export const IDENTITY_HASH_LENGTH = 16;
export const INTERFACE_ID_LENGTH = 8;
export const LINK_ID_LENGTH = 16;
export const PACKET_HASH_LENGTH = 32;
export const REQUEST_ID_LENGTH = 16;
export const REQUEST_PATH_HASH_LENGTH = 16;
export const RESOURCE_HASH_LENGTH = 32;
export const IDENTITY_SECRET_LENGTH = 64;
export const SAFE_INT_MIN = -9007199254740991;
export const SAFE_INT_MAX = 9007199254740991;
export const SAFE_UINT_MAX = 9007199254740991;
export const CAPABILITY_NAME_VALUES = Object.freeze([
    "Loopback",
    "TcpClient",
    "TcpServer",
    "Udp",
    "Serial",
    "Usb",
    "Bluetooth",
    "Wifi",
    "WebSocket",
    "BrowserRendezvous",
    "I2p",
    "Weave",
    "SuppliedPipe",
]);
export function isCapabilityName(value) {
    return typeof value === "string" && CAPABILITY_NAME_VALUES.includes(value);
}
export const LINK_CLOSED_REASON_VALUES = Object.freeze([
    "Timeout",
    "PeerClosed",
    "MalformedRtt",
]);
export function isLinkClosedReason(value) {
    return typeof value === "string" && LINK_CLOSED_REASON_VALUES.includes(value);
}
export const HOST_ROLE_NAME_VALUES = Object.freeze([
    "Endpoint",
    "Transport",
]);
export function isHostRoleName(value) {
    return typeof value === "string" && HOST_ROLE_NAME_VALUES.includes(value);
}
export const DELIVERY_EVIDENCE_KIND_VALUES = Object.freeze([
    "ExplicitProof",
    "ImplicitProof",
    "Response",
]);
export function isDeliveryEvidenceKind(value) {
    return typeof value === "string" && DELIVERY_EVIDENCE_KIND_VALUES.includes(value);
}
export const REQUEST_POLICY_VALUES = Object.freeze([
    "AllowNone",
    "AllowAll",
    "AllowList",
]);
export function isRequestPolicy(value) {
    return typeof value === "string" && REQUEST_POLICY_VALUES.includes(value);
}
export const PERSISTENCE_FLUSH_CAUSE_VALUES = Object.freeze([
    "Startup",
    "Interval",
    "RouteChange",
    "RatchetRotation",
    "Shutdown",
]);
export function isPersistenceFlushCause(value) {
    return typeof value === "string" && PERSISTENCE_FLUSH_CAUSE_VALUES.includes(value);
}
export const PERSISTENCE_FLUSH_TARGET_VALUES = Object.freeze([
    "RoutingState",
    "Ratchets",
]);
export function isPersistenceFlushTarget(value) {
    return typeof value === "string" && PERSISTENCE_FLUSH_TARGET_VALUES.includes(value);
}
export function balancedLimits() {
    return {
        pendingCommands: 256,
        applicationEvents: 1024,
        retainedEventBytes: 8388608,
        diagnostics: 1024,
    };
}
export const BACKEND_KIND_VALUES = Object.freeze([
    "Native",
    "Browser",
    "Cooperative",
]);
export function isBackendKind(value) {
    return typeof value === "string" && BACKEND_KIND_VALUES.includes(value);
}
export const INTERFACE_KIND_VALUES = Object.freeze([
    "AutoLan",
    "TcpClient",
    "TcpServer",
    "Udp",
    "Serial",
    "Kiss",
    "Ax25Kiss",
    "RNode",
    "MultiRNode",
    "Pipe",
    "BackboneClient",
    "BackboneServer",
    "I2p",
    "Weave",
    "AutomaticUsb",
    "AutomaticBluetoothLe",
    "WebSocketClient",
    "WebSocketServer",
    "BrowserRendezvous",
]);
export function isInterfaceKind(value) {
    return typeof value === "string" && INTERFACE_KIND_VALUES.includes(value);
}
export const INTERFACE_MODE_VALUES = Object.freeze([
    "Full",
    "PointToPoint",
    "AccessPoint",
    "Roaming",
    "Boundary",
    "Gateway",
    "Internal",
]);
export function isInterfaceMode(value) {
    return typeof value === "string" && INTERFACE_MODE_VALUES.includes(value);
}
export const WEB_SOCKET_FRAMING_SELECTION_VALUES = Object.freeze([
    "RawPacket",
    "Hdlc",
    "Kiss",
    "Auto",
]);
export function isWebSocketFramingSelection(value) {
    return typeof value === "string" && WEB_SOCKET_FRAMING_SELECTION_VALUES.includes(value);
}
export const INTERFACE_HEALTH_VALUES = Object.freeze([
    "Initializing",
    "Connected",
    "Degraded",
    "Reconnecting",
    "Failed",
    "Disconnected",
    "Disabled",
    "Unknown",
]);
export function isInterfaceHealth(value) {
    return typeof value === "string" && INTERFACE_HEALTH_VALUES.includes(value);
}
export const DISCOVERY_SCOPE_VALUES = Object.freeze([
    "Link",
    "Admin",
    "Site",
    "Organization",
    "Global",
]);
export function isDiscoveryScope(value) {
    return typeof value === "string" && DISCOVERY_SCOPE_VALUES.includes(value);
}
export const MULTICAST_ADDRESS_TYPE_VALUES = Object.freeze([
    "Temporary",
    "Permanent",
]);
export function isMulticastAddressType(value) {
    return typeof value === "string" && MULTICAST_ADDRESS_TYPE_VALUES.includes(value);
}
export const SERIAL_DATA_BITS_VALUES = Object.freeze([
    "Five",
    "Six",
    "Seven",
    "Eight",
]);
export function isSerialDataBits(value) {
    return typeof value === "string" && SERIAL_DATA_BITS_VALUES.includes(value);
}
export const SERIAL_PARITY_VALUES = Object.freeze([
    "None",
    "Even",
    "Odd",
]);
export function isSerialParity(value) {
    return typeof value === "string" && SERIAL_PARITY_VALUES.includes(value);
}
export const SERIAL_STOP_BITS_VALUES = Object.freeze([
    "One",
    "Two",
]);
export function isSerialStopBits(value) {
    return typeof value === "string" && SERIAL_STOP_BITS_VALUES.includes(value);
}
const HOST_OPERATION_NAMES = [
    "contractInfo",
    "backendInfo",
    "hostCreate",
    "hostRelease",
    "hostLifecycle",
    "hostSnapshot",
    "hostSnapshotRead",
    "hostSnapshotRelease",
    "hostIdentityHash",
    "hostDestinationCount",
    "hostDestinationHash",
    "hostAttachSuppliedPipe",
    "suppliedPipeClaimAttachment",
    "suppliedPipeNextOpenRequest",
    "suppliedPipeRegisterReadiness",
    "suppliedPipeInterruptWait",
    "suppliedPipeRelease",
    "suppliedPipeOpenRequestProvide",
    "suppliedPipeOpenRequestDecline",
    "suppliedPipeOpenRequestRelease",
    "hostBeginResourceUpload",
    "resourceUploadWrite",
    "resourceUploadIsWritable",
    "resourceUploadFinish",
    "resourceUploadAbort",
    "resourceUploadRelease",
    "hostStop",
    "commandWait",
    "commandRegisterReadiness",
    "commandInterruptWait",
    "commandRelease",
    "hostClaimApplicationEvents",
    "hostClaimDiagnostics",
    "eventStreamRegisterReadiness",
    "readinessRegistrationRelease",
    "eventStreamInterruptWait",
    "eventStreamRelease",
    "eventStreamNext",
    "eventRelease",
    "eventKind",
    "eventBytes",
    "eventString",
    "eventU64",
    "eventU128",
    "eventResourceStream",
    "resourceStreamRelease",
    "resourceStreamNext",
    "hostAnnounce",
    "hostSendSinglePacket",
    "hostCloseLink",
    "hostAttachTcpServer",
    "hostAttachTcpClient",
    "hostAttachUdp",
    "hostDetachInterface",
    "hostEstablishLink",
    "hostRequestPath",
    "hostIdentify",
    "hostSendLinkPacket",
    "hostRequest",
    "hostRespond",
    "hostSendResource",
    "hostSetLinkResourceStrategy",
    "hostSetDestinationResourceStrategy",
    "hostSendChannelMessage",
    "hostAllowRequester",
    "hostAttachInterface",
];
