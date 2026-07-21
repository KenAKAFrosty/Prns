import { Tag } from "./sdk/index.js";
import { boundedDetail } from "./presentation.js";
export function describeStartupFailure(outcome) {
    switch (outcome.tag) {
        case "WasmLoadFailed":
            return `WebAssembly load: ${outcome.data.detail}`;
        case "LxmfDisplayNameTooLong":
            return `LXMF display name is ${outcome.data.actual} bytes; the maximum is ${outcome.data.maximum}`;
        case "HostOperationFailed":
            return describeHostOperationFailure(outcome);
        case "HostApiUnavailable":
            return `${outcome.data.api} is unavailable in this browser`;
        case "IdentityStoreFailed":
            return `${outcome.data.operation} identity: ${outcome.data.detail}`;
        case "StoredIdentityInvalid":
            return `Stored identity: ${outcome.data.detail}`;
        case "EntropySourceFailed":
        case "InsufficientEntropy":
            return describeEntropyFailure(outcome);
        case "RuntimeRejected":
            return describeRuntimeRejected(outcome);
        default:
            return describeUnknownOutcome("runtime startup", outcome);
    }
}
export function describeUsbConnectFailure(outcome) {
    switch (outcome.tag) {
        case "HostOperationFailed":
            return describeHostOperationFailure(outcome);
        case "HostApiUnavailable":
            return `${outcome.data.api} is unavailable in this browser`;
        case "PermissionDenied":
            return `${outcome.data.stage}: ${outcome.data.detail}`;
        case "Cancelled":
            return `Cancelled during ${outcome.data.stage}`;
        case "AlreadyActive":
            return `Already active for ${outcome.data.target}`;
        case "UnsupportedDevice":
            return `Selected device lacks ${outcome.data.capability}`;
        case "ConnectionFailed":
            return `${outcome.data.stage}: ${outcome.data.detail}`;
        case "RuntimeRejected":
            return describeRuntimeRejected(outcome);
        default:
            return describeUnknownOutcome("USB Auto connection failure", outcome);
    }
}
export function describeUsbCloseFailure(outcome) {
    switch (outcome.tag) {
        case "HostOperationFailed":
            return describeHostOperationFailure(outcome);
        case "CloseFailed":
            return outcome.data.causes.map(describeCleanupFailure).join("; ");
        default:
            return describeUnknownOutcome("USB Auto close failure", outcome);
    }
}
export function describeAutoWifiFailure(outcome) {
    switch (outcome.tag) {
        case "HostApiUnavailable":
            return `${outcome.data.api} is unavailable in this browser`;
        case "PermissionDenied":
            return `${outcome.data.stage}: ${outcome.data.detail}`;
        case "AlreadyActive":
            return `Already active for ${outcome.data.target}`;
        case "SelectionIdentityUnavailable":
        case "DiscoveryFailed":
            return outcome.data.detail;
        case "RuntimeRejected":
            return describeRuntimeRejected(outcome);
        default:
            return describeUnknownOutcome("Auto Wi-Fi failure", outcome);
    }
}
export function describeSessionFailure(outcome) {
    switch (outcome.tag) {
        case "Disconnected":
            return outcome.data.detail;
        case "TransferFailed":
            return `${outcome.data.direction}: ${outcome.data.detail}`;
        case "ProtocolViolation":
            return `${outcome.data.protocol}: ${outcome.data.detail}`;
        case "UnsupportedFrame":
            return `${outcome.data.format} frame is unsupported`;
        case "FrameTooLarge":
            return `${outcome.data.length} bytes exceeds the ${outcome.data.maximum}-byte limit`;
        case "OutboundQueueFull":
            return `${outcome.data.capacity}-frame outbound queue is full`;
        case "CloseFailed":
            return outcome.data.causes.map(describeCleanupFailure).join("; ");
        case "UnexpectedSessionFailure":
            return outcome.data.detail;
        case "HostApiUnavailable":
        case "EntropySourceFailed":
        case "InsufficientEntropy":
            return describeEntropyFailure(outcome);
        case "RuntimeRejected":
            return describeRuntimeRejected(outcome);
        default:
            return describeUnknownOutcome("interface session failure", outcome);
    }
}
export function describeEntropyFailure(outcome) {
    switch (outcome.tag) {
        case "HostApiUnavailable":
            return `${outcome.data.api} is unavailable in this browser`;
        case "EntropySourceFailed":
            return outcome.data.detail;
        case "InsufficientEntropy":
            return `${outcome.data.actual} bytes received; ${outcome.data.minimum} required`;
        default:
            return describeUnknownOutcome("entropy failure", outcome);
    }
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
export function describeUnknownOutcome(context, _outcome) {
    return `${context} returned an outcome this playground does not recognize`;
}
function describeCleanupFailure(outcome) {
    switch (outcome.tag) {
        case "RuntimeDetachFailed":
            return `runtime detach: ${outcome.data.detail}`;
        case "TransportCloseFailed":
            return `transport close: ${outcome.data.detail}`;
        default:
            return describeUnknownOutcome("interface cleanup failure", outcome);
    }
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
