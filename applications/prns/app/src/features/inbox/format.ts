import * as Bindings from "@prns-internal/expo";
import type {
  Contact,
  LxmfDeliveryFailure,
  LxmfMessage,
  LxmfPeerSummary,
  LxmfText,
} from "@prns-internal/expo";

import { formatContactHash } from "@/features/contacts/format";

export function shortDestination(destination: Uint8Array): string {
  const encoded = formatContactHash(destination);
  return `${encoded.slice(0, 8)}…${encoded.slice(-6)}`;
}

export function peerLabel(
  destination: Uint8Array,
  peers: readonly LxmfPeerSummary[],
  contacts: readonly Contact[],
): string {
  const encoded = formatContactHash(destination);
  const alias = contacts.find(
    (contact) => formatContactHash(contact.destination) === encoded,
  )?.alias;
  if (alias !== undefined && alias.trim().length > 0) {
    return alias;
  }
  const announced = peers.find(
    (peer) => formatContactHash(peer.destination) === encoded,
  )?.displayName;
  if (announced !== undefined && announced.trim().length > 0) {
    return announced;
  }
  return shortDestination(destination);
}

export function messagePeer(message: LxmfMessage): Uint8Array {
  return message.direction === Bindings.LxmfDirection.Inbound
    ? message.source
    : message.destination;
}

export function textPresentation(value: LxmfText): {
  readonly text: string;
  readonly validUtf8: boolean;
} {
  if (value.tag === Bindings.LxmfText_Tags.Utf8) {
    return { text: value.inner.value, validUtf8: true };
  }
  return {
    text: "Unreadable text",
    validUtf8: false,
  };
}

export function deliveryLabel(message: LxmfMessage): string {
  switch (message.deliveryState.tag) {
    case Bindings.LxmfDeliveryState_Tags.Received:
      return "Received";
    case Bindings.LxmfDeliveryState_Tags.Queued:
      return message.deliveryState.inner.failedAttempts === 0n
        ? "Queued"
        : `Queued after ${attemptLabel(message.deliveryState.inner.failedAttempts)}`;
    case Bindings.LxmfDeliveryState_Tags.Sending:
      return message.deliveryState.inner.failedAttempts === 0n
        ? "Sending"
        : `Sending after ${attemptLabel(message.deliveryState.inner.failedAttempts)}`;
    case Bindings.LxmfDeliveryState_Tags.Delivered:
      return message.deliveryState.inner.rtt === undefined
        ? `Delivered at ${timestampLabel(message.deliveryState.inner.deliveredAt)}`
        : `Delivered in ${message.deliveryState.inner.rtt.toString()} ms at ${timestampLabel(message.deliveryState.inner.deliveredAt)}`;
    case Bindings.LxmfDeliveryState_Tags.Failed:
      return `Failed after ${attemptLabel(message.deliveryState.inner.failedAttempts)} — ${failureLabel(message.deliveryState.inner.lastFailure)}`;
    case Bindings.LxmfDeliveryState_Tags.Cancelled:
      return `Cancelled ${timestampLabel(message.deliveryState.inner.cancelledAt)}`;
  }
}

export function verificationLabel(message: LxmfMessage): string {
  switch (message.verification) {
    case Bindings.LxmfVerification.Verified:
      return "Verified source";
    case Bindings.LxmfVerification.SourceUnknown:
      return "Unverified — source identity unavailable";
    case Bindings.LxmfVerification.InvalidSignature:
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
    case Bindings.LxmfDeliveryFailure.NoRoute:
      return "no route";
    case Bindings.LxmfDeliveryFailure.LinkFailed:
      return "connection failed";
    case Bindings.LxmfDeliveryFailure.DeliveryTimedOut:
      return "delivery timed out";
    case Bindings.LxmfDeliveryFailure.LocalNodeStopped:
      return "this device went offline";
  }
}
