import { Link, Slot, useGlobalSearchParams, usePathname, useRouter } from "expo-router";
import { Modal, Pressable, ScrollView, StyleSheet, useWindowDimensions, View } from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";
import { useState } from "react";
import { identityKey } from "../identity-reference";
import { useInstallation } from "../runtime/InstallationProvider";
import { IncomingContacts } from "../runtime/IncomingContacts";
import { NavigationIcon } from "./navigation-icon";
import { InfoDisclosure } from "./InfoDisclosure";
import { useAppPalette } from "./theme";
import { Button, ListRow, Text } from "./primitives";
import { scalar } from "../navigation";
import { activityLabel } from "../runtime/identity-activity";

const destinations = [
  { path: "/conversations", label: "Conversations", icon: "inbox" },
  { path: "/contacts", label: "Contacts", icon: "contacts" },
  { path: "/nodes", label: "Nodes", icon: "nodes" },
  { path: "/more", label: "More", icon: "more" },
] as const;

export function Shell() {
  const palette = useAppPalette();
  const { width } = useWindowDimensions();
  const wide = width >= 768;
  const pathname = usePathname();
  const params = useGlobalSearchParams<{ owner?: string; group?: string }>();
  const router = useRouter();
  const installation = useInstallation();
  const [choosing, setChoosing] = useState(false);
  const selected = installation.records.find((record) => identityKey(record) === installation.selected);
  const inviteGroup = (["/invite", "/details", "/delivery"].some((suffix) => pathname.endsWith(suffix)) || /^\/conversations\/group\/[^/]+\/message\/[^/]+\/[^/]+$/.test(pathname)) && pathname.startsWith("/conversations/group/") ? scalar(params.group) : undefined;
  const parent = inviteGroup !== undefined && /^[0-9a-f]{64}$/.test(inviteGroup) ? { path: "/conversations/group/[group]", label: "Conversation", group: inviteGroup }
    : pathname.startsWith("/conversations/invitation/") || pathname === "/conversations/activity" ? { path: "/conversations", label: "Conversations" }
    : pathname.startsWith("/conversations/group/") || pathname === "/conversations/new" ? { path: "/conversations", label: "Conversations" }
    : pathname === "/nodes/phone" ? { path: "/nodes", label: "Nodes" }
    : pathname.startsWith("/contacts/") ? { path: "/contacts", label: "Contacts" }
    : pathname.startsWith("/more/identities/") ? { path: "/more/identities", label: "Identities" }
    : pathname.startsWith("/more/settings/") ? { path: "/more/settings", label: "Settings" }
    : pathname.startsWith("/more/") ? { path: "/more", label: "More" } : undefined;
  const navigation = <View accessibilityLabel="Primary navigation" style={[styles.navigation, wide ? styles.rail : styles.bottom, { backgroundColor: palette.surface, borderColor: palette.border }]}>
    {destinations.map((entry) => {
      const current = pathname === entry.path || pathname.startsWith(`${entry.path}/`);
      return <NavigationItem key={entry.path} entry={entry} current={current} wide={wide} owner={installation.selected} />;
    })}
  </View>;
  return <SafeAreaView style={[styles.root, { backgroundColor: palette.background }]}>
    <View style={[styles.header, { backgroundColor: palette.surface, borderColor: palette.border }]}>
      <Text style={styles.brand}>PRNS Chat</Text>
      <View style={styles.headerActions}>
        <Button size="compact" tone="quiet" title={selected?.metadata.localLabel ?? "Choose identity"} accessibilityLabel="Choose messaging identity" onPress={() => setChoosing(true)} />
        <InfoDisclosure label="About PRNS Chat"><Text>Choose a messaging identity to keep its contacts and conversations separate.</Text><Text>Keep PRNS Chat open for reliable delivery during this alpha. Use test data; saved messages are not yet encrypted at rest.</Text></InfoDisclosure>
      </View>
    </View>
    {installation.error !== undefined && <View style={{ padding: 14 }}><Text accessibilityRole="alert">{installation.error}</Text></View>}
    {installation.records.some((record) => identityKey(record) !== installation.selected &&
      ((installation.activity[identityKey(record)]?.unread ?? 0) > 0 || (installation.activity[identityKey(record)]?.invitations ?? 0) > 0)) &&
      <View style={{ paddingHorizontal: 18, paddingVertical: 8 }}><Button title="Other identities have unread messages or invitations" onPress={() => setChoosing(true)} /></View>}
    {installation.status?.latestIdentityDiagnostic !== undefined && <View style={{ padding: 14 }}><Text>An identity needs attention. Review its status in Messaging identities.</Text></View>}
    <IncomingContacts />
    {parent !== undefined && <View style={{ paddingHorizontal: 18, paddingTop: 10 }}>
      <Link href={{ pathname: parent.path, params: { ...(scalar(params.owner) === undefined ? {} : { owner: scalar(params.owner)! }), ...("group" in parent ? { group: parent.group } : {}) } }} asChild><Button title={parent.label} accessibilityLabel={`Back to ${parent.label}`} tone="quiet" size="compact" icon="back" /></Link>
    </View>}
    <View style={styles.body}>{wide && navigation}<View style={styles.content}><Slot /></View></View>
    {!wide && navigation}
    <Modal visible={choosing} transparent animationType="fade" onRequestClose={() => setChoosing(false)}>
      <View style={styles.scrim}><View accessibilityViewIsModal style={[styles.dialog, { backgroundColor: palette.surface }]}>
        <Text accessibilityRole="header" style={styles.brand}>Messaging identities</Text>
        <ScrollView contentContainerStyle={{ gap: 12 }}>
          {installation.records.map((record) => <View key={identityKey(record)}><ListRow title={record.metadata.localLabel} subtitle={identityKey(record) === installation.selected ? "Selected" : activityLabel(installation.activity[identityKey(record)])}
            onPress={() => { const owner = identityKey(record); installation.select(owner); setChoosing(false); router.replace({ pathname: "/conversations", params: { owner } }); }} />
          </View>)}
          <Button tone="quiet" title="Manage identities" onPress={() => { setChoosing(false); router.push("/more/identities"); }} />
        </ScrollView>
        <Button tone="quiet" title="Close identity chooser" onPress={() => setChoosing(false)} />
      </View></View>
    </Modal>
  </SafeAreaView>;
}

function NavigationItem({ entry, current, wide, owner }: {
  readonly entry: typeof destinations[number]; readonly current: boolean; readonly wide: boolean; readonly owner: string | undefined;
}) {
  const palette = useAppPalette();
  const [pressed, setPressed] = useState(false);
  const [focused, setFocused] = useState(false);
  return <View style={wide ? styles.railEntry : styles.bottomEntry}>
    <Link href={{ pathname: entry.path, params: owner === undefined ? {} : { owner } }} asChild>
      <Pressable accessibilityRole="tab" accessibilityState={{ selected: current }} accessibilityLabel={entry.label}
        style={styles.pressable} onPressIn={() => setPressed(true)} onPressOut={() => setPressed(false)}
        onFocus={() => setFocused(true)} onBlur={() => { setFocused(false); setPressed(false); }}>
        <View style={[styles.item, wide && styles.railItem, {
          backgroundColor: pressed ? palette.surfaceRaised : current ? palette.selected : "transparent",
          borderColor: focused ? palette.focus : "transparent",
        }]}>
          <View style={styles.icon}><NavigationIcon section={entry.icon} color={current ? palette.selectedText : palette.textMuted} /></View>
          <Text style={[styles.label, wide && styles.railLabel, { color: current ? palette.selectedText : palette.textMuted }]}>{entry.label}</Text>
        </View>
      </Pressable>
    </Link>
  </View>;
}

const styles = StyleSheet.create({
  root: { flex: 1 }, header: { paddingHorizontal: 18, paddingVertical: 10, borderBottomWidth: 1, gap: 12, flexDirection: "row", alignItems: "center", justifyContent: "space-between", flexWrap: "wrap" },
  brand: { fontSize: 22, lineHeight: 28, fontWeight: "700" }, body: { flex: 1, flexDirection: "row", minHeight: 0 }, content: { flex: 1, minWidth: 0 },
  navigation: { borderColor: "transparent", gap: 4, padding: 4 }, rail: { width: 210, borderRightWidth: 1 }, bottom: { flexDirection: "row", borderTopWidth: 1 },
  headerActions: { flexDirection: "row", alignItems: "center", gap: 8, flexShrink: 1 },
  bottomEntry: { flex: 1, minWidth: 0 }, railEntry: { width: "100%" }, pressable: { flexGrow: 1, width: "100%" },
  item: { flexGrow: 1, minHeight: 52, borderWidth: 1, borderRadius: 10, gap: 2, paddingVertical: 4, paddingHorizontal: 2, alignItems: "center", justifyContent: "flex-start" },
  icon: { width: 24, height: 24, flexShrink: 0, alignItems: "center", justifyContent: "center" },
  railItem: { flex: 0, flexDirection: "row", alignItems: "center", justifyContent: "flex-start", paddingHorizontal: 12, gap: 12 },
  label: { fontSize: 11, lineHeight: 16, fontWeight: "600", textAlign: "center", alignSelf: "stretch", flexShrink: 1 },
  railLabel: { flex: 1, alignSelf: "center", textAlign: "left" },
  scrim: { flex: 1, backgroundColor: "#0008", justifyContent: "center", alignItems: "center", padding: 24 }, dialog: { width: "100%", maxWidth: 480, maxHeight: "80%", borderRadius: 18, padding: 24, gap: 20 },
});
