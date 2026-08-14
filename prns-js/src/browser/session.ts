import { Tag, match_into } from "../casework.js";
import { describeHostError } from "./host_errors.js";
import type {
  InterfaceCleanupFailure,
  InterfaceCleanupFailures,
  InterfaceCloseOutcome,
  InterfaceSessionFailure,
  InterfaceSessionStatus,
} from "./interface_contract.js";

export function unexpectedSessionFailure(error: unknown): Extract<
  InterfaceSessionFailure,
  Tag<"UnexpectedSessionFailure", unknown>
> {
  return Tag("UnexpectedSessionFailure", { detail: describeHostError(error) });
}

export function closeFailed(
  causes: InterfaceCleanupFailures,
): Extract<InterfaceSessionFailure, Tag<"CloseFailed", unknown>> {
  return Tag("CloseFailed", { causes });
}

export function hasCleanupFailures(
  causes: readonly InterfaceCleanupFailure[],
): causes is InterfaceCleanupFailures {
  return causes.length > 0;
}

export function closedSessionOutcome(
  status: InterfaceSessionStatus,
): InterfaceCloseOutcome {
  return status.tag === "Failed" && status.data.tag === "CloseFailed"
    ? status.data
    : Tag("Closed");
}

export function describeInterfaceSessionFailure(
  failure: InterfaceSessionFailure,
): string {
  return match_into<string>().from(failure, {
    Disconnected: ({ detail }) => detail,
    UnexpectedSessionFailure: ({ detail }) => detail,
    EntropySourceFailed: ({ detail }) => detail,
    TransferFailed: ({ direction, detail }) =>
      `${direction} transfer: ${detail}`,
    ProtocolViolation: ({ protocol, detail }) => `${protocol}: ${detail}`,
    UnsupportedFrame: ({ format }) =>
      `unsupported ${format.toLowerCase()} frame`,
    FrameTooLarge: ({ length, maximum }) =>
      `frame is ${length} bytes; maximum is ${maximum}`,
    OutboundQueueFull: ({ capacity }) =>
      `outbound queue reached ${capacity} frames`,
    CloseFailed: ({ causes }) =>
      causes.map((cause) => cause.data.detail).join("; "),
    HostApiUnavailable: ({ api }) => `${api} is unavailable`,
    InsufficientEntropy: ({ actual, minimum }) =>
      `entropy source returned ${actual} bytes; minimum is ${minimum}`,
    RuntimeRejected: ({ operation, detail }) => `${operation}: ${detail}`,
  });
}

export function delay(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}
