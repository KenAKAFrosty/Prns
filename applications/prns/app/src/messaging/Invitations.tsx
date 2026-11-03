import { View } from "react-native";
import { prns_groups as groups } from "@prns/app-bindings";
import type { SavedContact } from "../contacts";
import { hex } from "../bytes";
import { useStyles } from "../ui";
import { Badge, Button, ListRow, Text } from "../ui/primitives";
import { Disclosure } from "../ui/Disclosure";
import { contactName, timestampLabel } from "./presentation";

export function invitationState(invitation: groups.GroupInvitation): string {
  switch (invitation.state) {
    case groups.GroupInvitationState.Pending: return "Awaiting response";
    case groups.GroupInvitationState.JoinRequested: return "Joining";
    case groups.GroupInvitationState.Joined: return "Joined";
    case groups.GroupInvitationState.Declined: return "Declined";
    case groups.GroupInvitationState.Expired: return "Expired";
    case groups.GroupInvitationState.Cancelled: return "Cancelled";
    case groups.GroupInvitationState.UnsupportedProfile: return "App update needed";
  }
}
export function InvitationRow({ invitation, contacts, onOpen }: {
  invitation: groups.GroupInvitation; contacts: readonly SavedContact[]; onOpen(): void;
}) {
  const peer = contactName(invitation.outgoing ? invitation.recipient : invitation.inviter, contacts);
  return <ListRow title={invitation.title} subtitle={`${invitation.outgoing ? "To" : "From"} ${peer}`}
    badge={<Badge>{invitationState(invitation)}</Badge>} accessibilityLabel={`Review invitation to ${invitation.title}`} onPress={onOpen} />;
}
export function InvitationDetails({ invitation, contacts, disabled, respond, cancel, dismiss, openGroup }: {
  invitation: groups.GroupInvitation; contacts: readonly SavedContact[]; disabled: boolean;
  respond(accept: boolean): void; cancel(): void; dismiss(): void; openGroup(): void;
}) {
  const styles = useStyles();
  return <View style={styles.section}>
    <Text accessibilityRole="header" style={styles.heading}>{invitation.title}</Text>
    <Badge>{invitationState(invitation)}</Badge>
    <Text>{invitation.outgoing ? "To" : "From"} {contactName(invitation.outgoing ? invitation.recipient : invitation.inviter, contacts)}</Text>
    {invitation.waitingForDelivery ? <Text>{invitation.outgoing
      ? "Joining will continue automatically when this phone’s pending deliveries finish or expire."
      : "Your acceptance is saved. The owner is finishing pending deliveries; joining will continue automatically."}</Text>
      : invitation.state === groups.GroupInvitationState.JoinRequested && <Text>{invitation.outgoing
        ? "Waiting for the invited person to finish joining. Keep both devices connected."
        : "Your acceptance is saved. Reconnect with the owner to finish joining."}</Text>}
    {invitation.state === groups.GroupInvitationState.Expired && <Text>{invitation.outgoing ? "Send a new invitation if this person should still join." : "Ask the group owner for a new invitation."}</Text>}
    <Text>Expires {timestampLabel(invitation.expiresAtSecs * 1000n)}</Text>
    <Disclosure label="Invitation identity details">
      <Text>Owner identity</Text><Text selectable style={styles.fingerprint}>{hex(invitation.inviter)}</Text>
      <Text>Invited identity</Text><Text selectable style={styles.fingerprint}>{hex(invitation.recipient)}</Text>
      <Text>Maximum members: {invitation.profile.maxMembers}</Text>
      <Text>Names alone do not verify a person. Compare the identity with someone you trust.</Text>
    </Disclosure>
    {!invitation.outgoing && invitation.state === groups.GroupInvitationState.Pending && <>
      <Text>Accept only if you recognize the owner. You will receive future group messages after joining.</Text>
      <Button tone="primary" title={`Accept ${invitation.title}`} disabled={disabled} onPress={() => respond(true)} />
      <Button title={`Decline ${invitation.title}`} disabled={disabled} onPress={() => respond(false)} />
    </>}
    {invitation.state === groups.GroupInvitationState.Joined && <Button tone="primary" title="Open conversation" onPress={openGroup} />}
    {invitation.canCancel && <Button title={`Cancel invitation to ${invitation.title}`} disabled={disabled} onPress={cancel} />}
    {invitation.canDismiss && <Button title={`Dismiss invitation to ${invitation.title}`} disabled={disabled} onPress={dismiss} />}
  </View>;
}
