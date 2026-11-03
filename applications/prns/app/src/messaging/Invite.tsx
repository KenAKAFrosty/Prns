import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { View } from "react-native";
import { prns_groups as groups } from "@prns/app-bindings";
import type { SavedContact } from "../contacts";
import { contactKey } from "../contacts";
import { hex } from "../bytes";
import { useStyles } from "../ui";
import { Badge, Button, ListRow, PageHeader, Text, TextInput } from "../ui/primitives";
import { MEMBER_PAGE, CATALOG_PAGE } from "./useConversation";
import { errorMessage } from "./presentation";

type Candidate = { identity: string; name: string; contact?: string };
type Review = { client: groups.ClientLike; key: string; member: boolean; invited: boolean; complete: boolean; error?: string };
const sameControl = (left: groups.GroupConversation["control"], right: groups.GroupConversation["control"]) => hex(left.era) === hex(right.era) && hex(left.head) === hex(right.head);
const identityBytes = (value: string) => Uint8Array.from(value.match(/../g)!, (part) => Number.parseInt(part, 16)).buffer;

/** A bounded review supplies hints. Native submission validates current authority. */
export function Invite({ client, conversation, contacts, active, disabled, revision, initialContact, invite }: {
  client: groups.ClientLike; conversation: groups.GroupConversation; contacts: readonly SavedContact[];
  active: boolean; disabled: boolean; revision: number; initialContact?: string | undefined; invite(identity: ArrayBuffer): void;
}) {
  const styles = useStyles();
  const [search, setSearch] = useState("");
  const [manual, setManual] = useState(false);
  const [fingerprint, setFingerprint] = useState("");
  const [candidate, setCandidate] = useState<Candidate>();
  const [review, setReview] = useState<Review>();
  const [reviewAttempt, setReviewAttempt] = useState(0);
  const group = hex(conversation.group), control = `${hex(conversation.control.era)}:${hex(conversation.control.head)}`;
  const selectedInitial = useRef(false);
  useLayoutEffect(() => { selectedInitial.current = false; setCandidate(undefined); setReview(undefined); }, [group, initialContact]);
  useEffect(() => {
    if (selectedInitial.current || initialContact === undefined) return;
    const contact = contacts.find((value) => contactKey(value) === initialContact);
    if (contact !== undefined) { selectedInitial.current = true; setCandidate({ identity: hex(contact.claims.identityHash), name: contact.alias || contact.claims.displayName, contact: initialContact }); }
  }, [group, initialContact, contacts]);
  const missing = candidate?.contact !== undefined && !contacts.some((value) => contactKey(value) === candidate.contact && hex(value.claims.identityHash) === candidate.identity);
  const key = candidate === undefined ? undefined : `${group}:${control}:${candidate.identity}`;
  useEffect(() => {
    if (!active || candidate === undefined || key === undefined || missing) return;
    const abort = new AbortController();
    const selected = candidate;
    void (async () => {
      let member = false, invited = false, membersComplete = false, invitationsComplete = false;
      try {
        let after: groups.GroupConversationMember["endpoint"] | undefined;
        // This is a review-work budget, not a group size limit. Unknown hints
        // never prevent an otherwise valid invitation from reaching the owner.
        for (let pageNumber = 0; pageNumber < 32 && !abort.signal.aborted; pageNumber++) {
          const page = await client.members(conversation.group, after, MEMBER_PAGE, { signal: abort.signal });
          if (!sameControl(page.control, conversation.control)) throw new Error("Membership changed. Choose this person again after the group updates.");
          member = page.members.some((value) => hex(value.identity) === selected.identity);
          if (member || page.next === undefined) { membersComplete = true; break; }
          after = page.next;
        }
        let invitationAfter: ArrayBuffer | undefined;
        for (let pageNumber = 0; pageNumber < 32 && !member && !abort.signal.aborted; pageNumber++) {
          const page = await client.invitations(BigInt(Math.floor(Date.now() / 1000)), invitationAfter, CATALOG_PAGE, { signal: abort.signal });
          invited = page.items.some((value) => hex(value.group) === group && value.outgoing && hex(value.recipient) === selected.identity &&
            (value.state === groups.GroupInvitationState.Pending || value.state === groups.GroupInvitationState.JoinRequested));
          if (invited || page.next === undefined) { invitationsComplete = true; break; }
          invitationAfter = page.next;
        }
        if (!abort.signal.aborted) setReview({ client, key, member, invited, complete: membersComplete && (member || invitationsComplete) });
      } catch (cause) { if (!abort.signal.aborted) setReview({ client, key, member: false, invited: false, complete: false, error: errorMessage(cause) }); }
    })();
    return () => abort.abort();
  }, [client, group, control, candidate?.identity, candidate?.contact, missing, key, active, revision, reviewAttempt]);
  const current = review?.client === client && review.key === key ? review : undefined;
  const query = search.trim().toLowerCase();
  const visible = contacts.filter((contact) => `${contact.alias} ${contact.claims.displayName} ${hex(contact.claims.identityHash)}`.toLowerCase().includes(query));
  return <View style={styles.section}>
    <PageHeader title="Invite people" subtitle={conversation.title} />
    {!conversation.capabilities.canInvite ? <Text>Only the group owner can invite people while the group is active.</Text> : <>
      {candidate === undefined && <>
      <TextInput accessibilityLabel="Search contacts to invite" placeholder="Search saved contacts" autoCorrect={false} value={search} onChangeText={setSearch} />
      {visible.map((contact) => <ListRow key={contactKey(contact)} title={contact.alias || contact.claims.displayName} subtitle={contact.claims.displayName}
        accessibilityLabel={`Choose ${contact.alias || contact.claims.displayName} to invite`} disabled={!active || disabled}
        onPress={() => setCandidate({ identity: hex(contact.claims.identityHash), name: contact.alias || contact.claims.displayName, contact: contactKey(contact) })} />)}
      {visible.length === 0 && <Text>{contacts.length === 0 ? "Save a contact card or enter an identity address to invite someone." : "No matching saved contacts."}</Text>}
      <Button size="compact" title={manual ? "Hide address entry" : "Enter an identity address"} onPress={() => setManual(!manual)} />
      {manual && <View style={styles.section}>
        <Text>Use the identity address shown on the person’s contact card. A contact card or QR code also includes their name and services.</Text>
        <TextInput accessibilityLabel="Invite identity address" placeholder="32 hexadecimal characters" autoCapitalize="none" autoCorrect={false}
          value={fingerprint} onChangeText={setFingerprint} maxLength={32} />
        <Button title="Review invitation" disabled={!active || disabled || !/^[0-9a-fA-F]{32}$/.test(fingerprint.trim())}
          onPress={() => setCandidate({ identity: fingerprint.trim().toLowerCase(), name: "Identity address" })} />
      </View>}
      </>}
      {candidate !== undefined && <View style={styles.card}>
        <Text accessibilityRole="header" style={styles.title}>Review recipient</Text><Text>{candidate.name}</Text>
        <Text selectable style={styles.fingerprint}>{candidate.identity}</Text>
        {missing ? <Text accessibilityRole="alert">This contact is no longer saved. Choose a recipient again.</Text>
          : current === undefined ? <Text>Checking membership and invitations…</Text>
          : current.member ? <Badge>Already a member</Badge>
          : current.invited ? <Badge>Invitation already pending</Badge>
          : <>{current.error !== undefined && <View><Text accessibilityRole="alert">{current.error}</Text><Button title="Retry recipient check" disabled={!active || disabled} onPress={() => setReviewAttempt((value) => value + 1)} /></View>}
            {!current.complete && <Text>Existing membership or invitations could not be fully checked. The group will validate your invitation.</Text>}
            <Text>Confirm this identity with the person you intend to invite. They choose whether to join.</Text></>}
        <Button tone="primary" title={`Send invitation to ${candidate.name}`} disabled={disabled || !active || missing || current === undefined || current.member || current.invited || current.error !== undefined}
          onPress={() => invite(identityBytes(candidate.identity))} />
        <Button title="Choose another person" disabled={disabled} onPress={() => setCandidate(undefined)} />
      </View>}
    </>}
  </View>;
}
