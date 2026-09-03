import type {
  Contact,
  LxmfDeliveryFailure,
  LxmfMessage,
  LxmfPeerSummary,
  LxmfText,
} from "@prns-internal/expo";
import type { DestinationHash } from "personal-rns/contract";

import { formatContactHash } from "@/features/contacts/format";

export function shortDestination(destination: Uint8Array): string {
  const encoded = formatContactHash(destination);
  return `${encoded.slice(0, 8)}…${encoded.slice(-6)}`;
}

export function peerLabel(
  destination: DestinationHash,
  peers: readonly LxmfPeerSummary[],
  contacts: readonly Contact[],
): string {
  const encoded = formatContactHash(destination);
  const alias = contacts.find(
    (contact) => formatContactHash(contact.destination) === encoded,
  )?.alias;
  if (alias !== null && alias !== undefined && alias.trim().length > 0) {
    return alias;
  }
  const announced = peers.find(
    (peer) => formatContactHash(peer.destination) === encoded,
  )?.displayName;
  if (announced !== null && announced !== undefined && announced.trim().length > 0) {
    return announced;
  }
  return shortDestination(destination);
}

export function messagePeer(message: LxmfMessage): DestinationHash {
  return message.direction === "inbound" ? message.source : message.destination;
}

export function textPresentation(value: LxmfText): {
  readonly text: string;
  readonly validUtf8: boolean;
} {
  if (value.type === "utf8") {
    return { text: value.value, validUtf8: true };
  }
  const preview = Array.from(value.bytes.slice(0, 12), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join(" ");
  const suffix = value.bytes.length > 12 ? " …" : "";
  return {
    text: `Invalid UTF-8 (${value.bytes.length} bytes: ${preview}${suffix})`,
    validUtf8: false,
  };
}

export function deliveryLabel(message: LxmfMessage): string {
  switch (message.deliveryState.type) {
    case "received":
      return "Received";
    case "queued":
      return message.deliveryState.failedAttempts === 0n
        ? "Queued — stored durably and awaiting send"
        : `Queued — stored durably after ${attemptLabel(message.deliveryState.failedAttempts)}`;
    case "sending":
      return message.deliveryState.failedAttempts === 0n
        ? "Sending — waiting for transport proof"
        : `Sending after ${attemptLabel(message.deliveryState.failedAttempts)} — waiting for transport proof`;
    case "delivered":
      return message.deliveryState.rtt === null
        ? `Delivered — transport proof received at ${timestampLabel(message.deliveryState.deliveredAt)}`
        : `Delivered — transport proof received in ${message.deliveryState.rtt.toString()} ms at ${timestampLabel(message.deliveryState.deliveredAt)}`;
    case "failed":
      return `Failed after ${attemptLabel(message.deliveryState.failedAttempts)} — ${failureLabel(message.deliveryState.lastFailure)}`;
    case "cancelled":
      return `Cancelled ${timestampLabel(message.deliveryState.cancelledAt)}`;
  }
}

export function verificationLabel(message: LxmfMessage): string {
  switch (message.verification) {
    case "verified":
      return "Verified source";
    case "sourceUnknown":
      return "Unverified — source identity unavailable";
    case "invalidSignature":
      return "Unverified — invalid signature";
  }
}

export function timestampLabel(timestamp: bigint): string {
  if (timestamp <= BigInt(Number.MAX_SAFE_INTEGER)) {
    const rendered = new Date(Number(timestamp));
    if (!Number.isNaN(rendered.valueOf())) {
      return rendered.toLocaleString();
    }
  }
  return `${timestamp.toString()} ms since Unix epoch`;
}

function attemptLabel(failedAttempts: bigint): string {
  return `${failedAttempts.toString()} failed ${failedAttempts === 1n ? "attempt" : "attempts"}`;
}

function failureLabel(failure: LxmfDeliveryFailure): string {
  switch (failure) {
    case "noRoute":
      return "no route";
    case "linkFailed":
      return "Link failed";
    case "deliveryTimedOut":
      return "delivery proof timed out";
    case "localNodeStopped":
      return "local node stopped";
  }
}
