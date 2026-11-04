import { prns_groups as groups } from "@prns/app-bindings";
import type { SavedContact } from "../contacts";
import { hex } from "../bytes";
import { contactName } from "./presentation";

export type ActionDestination = { screen: "create" } | { screen: "conversation" | "invite"; group: string } | { screen: "invitation"; invitation: string };
export function actionDestination(operation: groups.GroupOperation): ActionDestination {
  const intent = operation.request.intent;
  switch (intent.tag) {
    case groups.GroupIntent_Tags.Create: {
      if (operation.state.tag === groups.GroupOperationState_Tags.Completed) {
        const outcome = operation.state.inner[0];
        if (outcome.tag === groups.GroupOutcome_Tags.Created) return { screen: "conversation", group: hex(outcome.inner[0]) };
      }
      return { screen: "create" };
    }
    case groups.GroupIntent_Tags.Respond:
    case groups.GroupIntent_Tags.CancelInvitation:
    case groups.GroupIntent_Tags.DismissInvitation: return { screen: "invitation", invitation: hex(intent.inner.invitation) };
    case groups.GroupIntent_Tags.Publish: return { screen: "conversation", group: hex(intent.inner[0].group) };
    case groups.GroupIntent_Tags.Invite: return { screen: "invite", group: hex(intent.inner.group) };
    default: return { screen: "conversation", group: hex(intent.inner.group) };
  }
}
export function actionContext(operation: groups.GroupOperation, conversations: readonly groups.GroupConversationSummary[], invitations: readonly groups.GroupInvitation[], contacts: readonly SavedContact[]): string {
  const target = actionDestination(operation), intent = operation.request.intent;
  if (target.screen === "create") return intent.tag === groups.GroupIntent_Tags.Create ? intent.inner.title : "New conversation";
  if (target.screen === "invitation") return invitations.find((value) => hex(value.id) === target.invitation)?.title ?? `Invitation ${target.invitation.slice(0, 8)}`;
  const title = conversations.find((value) => hex(value.group) === target.group)?.title ?? `Conversation ${target.group.slice(0, 8)}`;
  return intent.tag === groups.GroupIntent_Tags.Invite ? `${contactName(intent.inner.recipient, contacts)} · ${title}` : title;
}
