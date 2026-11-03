import { useCallback, useMemo } from "react";
import { View } from "react-native";
import { useStyles } from "../ui";
import { Button, Text } from "../ui/primitives";
import type { prns_groups_core as protocol, prns_groups as groups } from "@prns/app-bindings";
import type { SavedContact } from "../contacts";
import { hex } from "../bytes";
import { contactName } from "./presentation";
import { usePaged } from "./usePaged";
import { groupId, MEMBER_PAGE } from "./useConversation";
import { Paging } from "./Paging";
function controlKey(control: groups.GroupConversation["control"]): string {
  return `${hex(control.era)}:${hex(control.head)}`;
}
export function Members({ client, conversation, contacts, active, revision, disabled, refreshDetail, remove }: {
  client: groups.ClientLike; conversation: groups.GroupConversation; contacts: readonly SavedContact[];
  active: boolean; revision: number; disabled: boolean; refreshDetail: () => void;
  remove: (member: protocol.GroupEndpoint) => void;
}) {
  const styles = useStyles();
  const control = controlKey(conversation.control);
  const group = hex(conversation.group);
  const requestGroup = useMemo(() => groupId(group), [group]);
  const read = useCallback(async (after: protocol.GroupEndpoint | undefined, signal: AbortSignal) => {
    const page = await client.members(requestGroup, after, MEMBER_PAGE, { signal });
    if (controlKey(page.control) !== control) {
      // Never combine different authenticated rosters. Exact detail refresh
      // changes this hook's scope and restarts at the first member page.
      if (!signal.aborted) refreshDetail();
      throw new Error("Membership changed. Refreshing group details.");
    }
    return { items: page.members, next: page.next };
  }, [client, requestGroup, control, refreshDetail]);
  const page = usePaged({ owner: client, scope: `${group}:${control}`, active, revision, read });
  return <View style={styles.section}>
    {page.rows.map((member) => <View key={`${hex(member.endpoint.id)}:${member.endpoint.incarnation}`} style={styles.card}>
      <Text>{member.isLocal ? "You" : contactName(member.identity, contacts)}{member.isOwner ? " · owner" : ""}</Text>
      <Text selectable style={styles.fingerprint}>{hex(member.identity)}</Text>
      {conversation.capabilities.canRemoveMembers && !member.isLocal && !member.isOwner &&
        <Button title={`Remove ${contactName(member.identity, contacts)}`} disabled={disabled || !page.loaded || page.error !== undefined || !active}
          onPress={() => { if (active && !disabled && page.loaded && page.error === undefined) remove(member.endpoint); }} />}
    </View>)}
    <Paging label="Members" page={page} active={active} />
  </View>;
}
