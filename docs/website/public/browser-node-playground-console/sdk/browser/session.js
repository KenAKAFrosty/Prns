import { Tag, match_into } from "../casework.js";
import { describeHostError } from "./host_errors.js";
export function unexpectedSessionFailure(error) {
    return Tag("UnexpectedSessionFailure", { detail: describeHostError(error) });
}
export function closeFailed(causes) {
    return Tag("CloseFailed", { causes });
}
export function hasCleanupFailures(causes) {
    return causes.length > 0;
}
export function closedSessionOutcome(status) {
    return status.tag === "Failed" && status.data.tag === "CloseFailed"
        ? status.data
        : Tag("Closed");
}
export function describeInterfaceSessionFailure(failure) {
    return match_into().from(failure, {
        Disconnected: ({ detail }) => detail,
        UnexpectedSessionFailure: ({ detail }) => detail,
        EntropySourceFailed: ({ detail }) => detail,
        TransferFailed: ({ direction, detail }) => `${direction} transfer: ${detail}`,
        ProtocolViolation: ({ protocol, detail }) => `${protocol}: ${detail}`,
        UnsupportedFrame: ({ format }) => `unsupported ${format.toLowerCase()} frame`,
        FrameTooLarge: ({ length, maximum }) => `frame is ${length} bytes; maximum is ${maximum}`,
        OutboundQueueFull: ({ capacity }) => `outbound queue reached ${capacity} frames`,
        CloseFailed: ({ causes }) => causes.map((cause) => cause.data.detail).join("; "),
        HostApiUnavailable: ({ api }) => `${api} is unavailable`,
        InsufficientEntropy: ({ actual, minimum }) => `entropy source returned ${actual} bytes; minimum is ${minimum}`,
        RuntimeRejected: ({ operation, detail }) => `${operation}: ${detail}`,
    });
}
export function delay(ms) {
    return new Promise((resolve) => {
        setTimeout(resolve, ms);
    });
}
