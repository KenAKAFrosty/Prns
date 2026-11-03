import { useAppPalette } from "../ui/theme";
import { Button, Text } from "../ui/primitives";
import { InfoDisclosure } from "../ui/InfoDisclosure";
import { useEffect, useRef, useState } from "react";
import { ActivityIndicator, Pressable, StyleSheet, View } from "react-native";
import type { prns_groups as groups } from "@prns/app-bindings";
import type { SavedContact } from "../contacts";
import { hex } from "../bytes";
import { deviceClock } from "./clock";
import { contactName, errorMessage, mergeMessages, messageKey, receiptText, timestampLabel } from "./presentation";

export function History({ client, conversation, contacts, revision, active, onViewed, onOpenMessage }: {
  client: groups.ClientLike; conversation: groups.GroupConversation; contacts: readonly SavedContact[]; revision: number | string; active: boolean; onViewed?: (heads: readonly groups.GroupChangeToken[]) => void;
  onOpenMessage?: ((group: string, incarnation: string, messageId: string) => void) | undefined;
}) {
  const styles = useStyles();
  // Native decoding creates fresh buffers; restart only when their values change.
  const groupKey = hex(conversation.group);
  const controlKey = `${hex(conversation.control.era)}:${hex(conversation.control.head)}`;
  const mounted = useRef(true);
  const loadedSource = useRef<{ client: groups.ClientLike; group: string } | undefined>(undefined);
  const navigation = useRef({ client, groupKey, active, onOpenMessage });
  navigation.current = { client, groupKey, active, onOpenMessage };
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const [messages, setMessages] = useState<groups.GroupTextMessage[]>([]);
  const [before, setBefore] = useState<groups.GroupRecordCursor>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [reload, setReload] = useState(0);
  const incarnation = useRef<string | undefined>(undefined);
  const headAnchors = useRef(new Map<string, groups.GroupChangeToken>());
  const generation = useRef(0);
  const olderRequest = useRef<AbortController | undefined>(undefined);
  const [loaded, setLoaded] = useState(false);
  const [viewedHeads, setViewedHeads] = useState<readonly groups.GroupChangeToken[]>([]);
  // Record local opening only after the page has reached the rendered view.
  // This is not evidence that a person read it or any network receipt.
  useEffect(() => { if (active && loaded) onViewed?.(viewedHeads); }, [active, loaded, onViewed, viewedHeads]);
  useEffect(() => {
    const ticket = ++generation.current;
    olderRequest.current?.abort();
    olderRequest.current = undefined;
    if (!active) { setBusy(false); return; }
    const abort = new AbortController();
    setBusy(true);
    void client.messages(conversation.group, undefined, 50, deviceClock(), { signal: abort.signal }).then((page) => {
      if (ticket !== generation.current || abort.signal.aborted) return;
      const current = hex(page.currentIncarnation);
      incarnation.current = current;
      // Refresh from one contiguous native page. Do not splice a new head into
      // old pages: a burst larger than one page could leave an invisible gap.
      loadedSource.current = { client, group: groupKey };
      setMessages(page.messages);
      headAnchors.current = new Map(page.visitedHeads.map((head) => [hex(head.incarnation), head]));
      setViewedHeads(page.visitedHeads);
      setBefore(page.nextBefore);
      setError(undefined);
      setLoaded(true);
    }).catch((cause: unknown) => {
      if (!abort.signal.aborted && ticket === generation.current) setError(errorMessage(cause));
    }).finally(() => { if (!abort.signal.aborted && ticket === generation.current) setBusy(false); });
    return () => { abort.abort(); olderRequest.current?.abort(); };
  }, [client, groupKey, controlKey, conversation.revision, revision, active, reload]);
  const older = async () => {
    if (busy || olderRequest.current !== undefined || before === undefined || !active) return;
    const ticket = generation.current;
    const abort = new AbortController();
    // Claim synchronously: React's busy state is not visible to a second press
    // in the same tick. Only this exact request may release the claim.
    olderRequest.current = abort;
    setBusy(true);
    try {
      const page = await client.messages(conversation.group, before, 50, deviceClock(), { signal: abort.signal });
      if (abort.signal.aborted || ticket !== generation.current) return;
      if (hex(page.currentIncarnation) !== incarnation.current) {
        setReload((value) => value + 1);
        return;
      }
      setMessages((previous) => mergeMessages(page.messages, previous));
      // A page before an older cursor can report a newer feed head for rows it
      // did not return. Preserve each namespace's first-visit anchor until a
      // fresh newest-page traversal actually includes that newer head.
      for (const head of page.visitedHeads) {
        if (!headAnchors.current.has(hex(head.incarnation))) headAnchors.current.set(hex(head.incarnation), head);
      }
      setViewedHeads([...headAnchors.current.values()]);
      setBefore(page.nextBefore); // Empty text page can still have earlier generic records.
      setError(undefined);
    } catch (cause) { if (!abort.signal.aborted && ticket === generation.current) setError(errorMessage(cause)); }
    finally {
      if (olderRequest.current === abort) {
        olderRequest.current = undefined;
        if (!abort.signal.aborted && ticket === generation.current) setBusy(false);
      }
    }
  };
  return <View style={styles.section}>
    <View style={styles.header}>
      <Text accessibilityRole="header" style={styles.title}>Messages</Text>
      <InfoDisclosure label="About message history"><Text>Messages stay on this phone until you delete them. A device confirmation means another device saved the message; it does not mean someone read it.</Text></InfoDisclosure>
    </View>
    {error !== undefined && <View><Text accessibilityRole="alert">{error}</Text><Button title="Reload messages" onPress={() => setReload((value) => value + 1)} /></View>}
    {before !== undefined && <Button size="compact" title="Load earlier messages" disabled={busy || !active} onPress={() => { void older(); }} />}
    {busy && !loaded && <ActivityIndicator accessibilityLabel="Loading messages" />}
    {loaded && messages.length === 0 && <Text>No text messages on the loaded pages.</Text>}
    {[...messages].reverse().map((message) => {
      const author = message.outgoing ? "You" : contactName(message.author, contacts);
      const incarnation = hex(message.position.incarnation), id = hex(message.id);
      const available = active && onOpenMessage !== undefined && loadedSource.current?.client === client && loadedSource.current.group === groupKey;
      return <Pressable key={messageKey(message)} accessibilityRole="button" accessibilityLabel={`Message from ${author}: ${message.text}`}
        accessibilityHint="Opens message details" accessibilityValue={{ text: receiptText(message) }} accessibilityState={{ disabled: !available }} disabled={!available}
        onPress={() => {
          const current = navigation.current;
          if (mounted.current && available && current.active && current.client === client && current.groupKey === groupKey && current.onOpenMessage === onOpenMessage)
            onOpenMessage?.(groupKey, incarnation, id);
        }}
        style={({ pressed }) => [styles.message, message.outgoing && styles.outgoing, pressed && styles.pressed]}>
        <Text style={styles.author}>{author}</Text>
        <Text>{message.text}</Text>
        <Text style={styles.evidence}>{timestampLabel(message.sentAtMillis)}</Text>
        <Text style={styles.evidence}>{receiptText(message)}</Text>
      </Pressable>;
    })}
  </View>;
}
function useStyles() {
  const palette = useAppPalette();
  return StyleSheet.create({
  section: { gap: 12 }, header: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: 8 }, title: { fontSize: 20, fontWeight: "600" },
  message: { maxWidth: "92%", alignSelf: "flex-start", padding: 12, gap: 6, borderRadius: 10, backgroundColor: palette.surfaceRaised },
  pressed: { opacity: 0.78 },
  outgoing: { alignSelf: "flex-end", backgroundColor: palette.selected }, author: { fontWeight: "600" }, evidence: { fontSize: 12, color: palette.textMuted },
  });
}
