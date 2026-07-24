import { Tag, match_into } from "./sdk/index.js";
import { boundedDetail } from "./presentation.js";
export function describeStartupFailure(outcome) {
    return match_into().from(outcome, {
        WasmLoadFailed: ({ detail }) => `WebAssembly load: ${detail}`,
        LxmfDisplayNameTooLong: ({ actual, maximum }) => `LXMF display name is ${actual} bytes; the maximum is ${maximum}`,
        HostOperationFailed: ({ operation, detail }) => `${operation}: ${detail}`,
        ContractMismatch: ({ actualAbi, actualProductVersion, requiredAbi, requiredProductVersion, }) => `Host contract ${actualAbi}/${actualProductVersion} ` +
            `does not match ${requiredAbi}/${requiredProductVersion}`,
        HostApiUnavailable: ({ api }) => `${api} is unavailable in this browser`,
        IdentityStoreFailed: ({ operation, detail }) => `${operation} identity: ${detail}`,
        StoredIdentityInvalid: ({ detail }) => `Stored identity: ${detail}`,
        EntropySourceFailed: ({ detail }) => detail,
        InsufficientEntropy: ({ actual, minimum }) => `${actual} bytes received; ${minimum} required`,
        RuntimeRejected: ({ operation, detail }) => `${operation}: ${detail}`,
    });
}
export function describeUsbConnectFailure(outcome) {
    return match_into().from(outcome, {
        HostOperationFailed: ({ operation, detail }) => `${operation}: ${detail}`,
        HostApiUnavailable: ({ api }) => `${api} is unavailable in this browser`,
        PermissionDenied: ({ stage, detail }) => `${stage}: ${detail}`,
        Cancelled: ({ stage }) => `Cancelled during ${stage}`,
        AlreadyActive: ({ target }) => `Already active for ${target}`,
        UnsupportedDevice: ({ capability }) => `Selected device lacks ${capability}`,
        ConnectionFailed: ({ stage, detail }) => `${stage}: ${detail}`,
        RuntimeRejected: ({ operation, detail }) => `${operation}: ${detail}`,
    });
}
export function describeUsbCloseFailure(outcome) {
    return match_into().from(outcome, {
        HostOperationFailed: ({ operation, detail }) => `${operation}: ${detail}`,
        CloseFailed: ({ causes }) => causes.map(describeCleanupFailure).join("; "),
    });
}
export function describeAutoWifiFailure(outcome) {
    return match_into().from(outcome, {
        HostApiUnavailable: ({ api }) => `${api} is unavailable in this browser`,
        PermissionDenied: ({ stage, detail }) => `${stage}: ${detail}`,
        AlreadyActive: ({ target }) => `Already active for ${target}`,
        SelectionIdentityUnavailable: ({ detail }) => detail,
        DiscoveryFailed: ({ detail }) => detail,
        RuntimeRejected: ({ operation, detail }) => `${operation}: ${detail}`,
    });
}
export function describeSessionFailure(outcome) {
    return match_into().from(outcome, {
        Disconnected: ({ detail }) => detail,
        TransferFailed: ({ direction, detail }) => `${direction}: ${detail}`,
        ProtocolViolation: ({ protocol, detail }) => `${protocol}: ${detail}`,
        UnsupportedFrame: ({ format }) => `${format} frame is unsupported`,
        FrameTooLarge: ({ length, maximum }) => `${length} bytes exceeds the ${maximum}-byte limit`,
        OutboundQueueFull: ({ capacity }) => `${capacity}-frame outbound queue is full`,
        CloseFailed: ({ causes }) => causes.map(describeCleanupFailure).join("; "),
        UnexpectedSessionFailure: ({ detail }) => detail,
        HostApiUnavailable: ({ api }) => `${api} is unavailable in this browser`,
        EntropySourceFailed: ({ detail }) => detail,
        InsufficientEntropy: ({ actual, minimum }) => `${actual} bytes received; ${minimum} required`,
        RuntimeRejected: ({ operation, detail }) => `${operation}: ${detail}`,
    });
}
export function describeEntropyFailure(outcome) {
    return match_into().from(outcome, {
        HostApiUnavailable: ({ api }) => `${api} is unavailable in this browser`,
        EntropySourceFailed: ({ detail }) => detail,
        InsufficientEntropy: ({ actual, minimum }) => `${actual} bytes received; ${minimum} required`,
    });
}
export function describeRuntimeRejected(outcome) {
    return `${outcome.data.operation}: ${outcome.data.detail}`;
}
export function hostOperationFailed(operation, error) {
    return Tag("HostOperationFailed", {
        operation,
        detail: describeHostError(error),
    });
}
export function describeHostOperationFailure(outcome) {
    return `${outcome.data.operation}: ${outcome.data.detail}`;
}
function describeCleanupFailure(outcome) {
    return match_into().from(outcome, {
        RuntimeDetachFailed: ({ detail }) => `runtime detach: ${detail}`,
        TransportCloseFailed: ({ detail }) => `transport close: ${detail}`,
    });
}
export function describeHostError(error) {
    if (error instanceof DOMException) {
        return boundedDetail(`${error.name}: ${error.message}`);
    }
    if (error instanceof Error) {
        return boundedDetail(`${error.name}: ${error.message}`);
    }
    if (typeof error === "string") {
        return boundedDetail(error);
    }
    return "The browser returned an opaque host failure";
}
