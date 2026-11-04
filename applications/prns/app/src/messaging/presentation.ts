import { prns_groups as groups, prns_groups_core as protocol } from "@prns/app-bindings";
import { hex } from "../bytes";
import type { SavedContact } from "../contacts";

export function contactName(identity: ArrayBuffer, contacts: readonly SavedContact[]): string {
  const contact = contacts.find((entry) => hex(entry.claims.identityHash) === hex(identity));
  return contact?.alias || contact?.claims.displayName || "Unknown contact";
}
export function actionName(intent: groups.GroupIntent): string {
  switch (intent.tag) {
    case groups.GroupIntent_Tags.Create: return `Create “${intent.inner.title}”`;
    case groups.GroupIntent_Tags.Publish: return "Send message";
    case groups.GroupIntent_Tags.Invite: return "Invite contact";
    case groups.GroupIntent_Tags.Respond: return intent.inner.accept ? "Accept invitation" : "Decline invitation";
    case groups.GroupIntent_Tags.CancelInvitation: return "Cancel invitation";
    case groups.GroupIntent_Tags.DismissInvitation: return "Dismiss invitation";
    case groups.GroupIntent_Tags.RemoveMember: return "Remove member";
    case groups.GroupIntent_Tags.Leave: return "Leave group";
    case groups.GroupIntent_Tags.Close: return "Close group for everyone";
    case groups.GroupIntent_Tags.DeleteLocal: return "Delete local history";
  }
}
export function terminal(operation: groups.GroupOperation): boolean {
  return operation.state.tag === groups.GroupOperationState_Tags.Completed || operation.state.tag === groups.GroupOperationState_Tags.Rejected;
}
export function operationStatus(operation: groups.GroupOperation): string {
  switch (operation.state.tag) {
    case groups.GroupOperationState_Tags.Pending: return "Saved on this device; waiting to run.";
    case groups.GroupOperationState_Tags.Dispatched: return "Started; its result is still being checked.";
    case groups.GroupOperationState_Tags.Uncertain: return "Result uncertain. Keep this request and check it again; do not repeat the action.";
    case groups.GroupOperationState_Tags.Completed: return "Saved on this device. This does not mean other devices have received or read it.";
    case groups.GroupOperationState_Tags.Rejected: return `Not applied: ${rejection(operation.state.inner[0])}`;
  }
}
function rejection(reason: protocol.GroupRejectionReason): string {
  switch (reason) {
    case protocol.GroupRejectionReason.StorageCapacity: return "Local storage is full. Delete unneeded local history before trying a new action.";
    case protocol.GroupRejectionReason.StaleMembership: return "Membership changed. Review the group before trying a new action.";
    default: return protocol.GroupRejectionReason[reason].replace(/([a-z])([A-Z])/g, "$1 $2");
  }
}
export function errorMessage(error: unknown): string {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "object" && error !== null && "tag" in error) {
    return `Group service: ${String(error.tag)}. Saved requests have been kept.`;
  }
  return String(error);
}
export function receiptText(message: groups.GroupTextMessage): string {
  const evidence = !message.outgoing || message.recipientCount === 0n ? "Saved on this phone"
    : message.receivedCount === 0n ? "Waiting for device confirmation"
      : `Saved by ${message.receivedCount} of ${message.recipientCount} devices`;
  return message.membershipChanged ? `${evidence} · membership changed` : evidence;
}
export function messageKey(message: groups.GroupTextMessage): string {
  return `${hex(message.position.incarnation)}:${hex(message.id)}`;
}
export function mergeMessages(older: readonly groups.GroupTextMessage[], latest: readonly groups.GroupTextMessage[]): groups.GroupTextMessage[] {
  const records = new Map(latest.map((message) => [messageKey(message), message]));
  for (const message of older) if (!records.has(messageKey(message))) records.set(messageKey(message), message);
  // Native insertion provenance is authoritative within an incarnation. Across
  // incarnations preserve the archive's page order rather than compare clocks.
  return [...records.values()];
}

/** Sender time is display metadata; native insertion provenance orders history. */
export function timestampLabel(milliseconds: bigint): string {
  if (milliseconds < 0n || milliseconds > 8_640_000_000_000_000n) return "Time unavailable";
  const date = new Date(Number(milliseconds));
  return Number.isNaN(date.getTime()) ? "Time unavailable" : date.toLocaleString();
}
