import { Link, Slot, usePathname } from "expo-router";
import type { PropsWithChildren } from "react";
import { Pressable, StyleSheet, Text, useWindowDimensions, View } from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";

import { navigationEntries, type ScreenCatalogEntry } from "@/navigation/catalog";
import { useScaffoldState } from "@/state/scaffold-state-context";
import { Badge } from "./primitives";
import { layoutModeForWidth, radius, space, useAppPalette } from "./theme";

export function ShellLayout() {
  const { width } = useWindowDimensions();
  const layout = layoutModeForWidth(width);
  const { state } = useScaffoldState();
  const entries = navigationEntries(
    layout === "compact" ? "phone" : "wide",
    state.showUnavailableFeatures,
  );
  const pathname = usePathname();
  const active = entries
    .filter((entry) => pathname === entry.path || pathname.startsWith(`${entry.path}/`))
    .sort((left, right) => right.path.length - left.path.length)[0];

  return (
    <SafeAreaView style={[styles.safeArea, { backgroundColor: useAppPalette().background }]}>
      <ShellHeader />
      <View style={styles.shellBody}>
        {layout === "compact" ? null : <NavigationRail active={active} entries={entries} />}
        <View style={styles.content}>
          <Slot />
        </View>
        {layout === "wide" ? <ContextPanel active={active} /> : null}
      </View>
      {layout === "compact" ? <BottomNavigation active={active} entries={entries} /> : null}
    </SafeAreaView>
  );
}

function ShellHeader() {
  const palette = useAppPalette();
  return (
    <View
      style={[styles.header, { backgroundColor: palette.surface, borderColor: palette.border }]}
    >
      <Text accessibilityRole="header" style={[styles.brand, { color: palette.text }]}>
        prns
      </Text>
      <Badge>Development preview</Badge>
    </View>
  );
}

function NavigationRail({
  active,
  entries,
}: {
  readonly active: ScreenCatalogEntry | undefined;
  readonly entries: readonly ScreenCatalogEntry[];
}) {
  const palette = useAppPalette();
  return (
    <View
      accessibilityLabel="Primary navigation"
      style={[styles.rail, { backgroundColor: palette.surface, borderColor: palette.border }]}
    >
      {entries.map((entry) => (
        <ShellNavigationItem active={active?.id === entry.id} entry={entry} key={entry.id} />
      ))}
    </View>
  );
}

function BottomNavigation({
  active,
  entries,
}: {
  readonly active: ScreenCatalogEntry | undefined;
  readonly entries: readonly ScreenCatalogEntry[];
}) {
  const palette = useAppPalette();
  return (
    <View
      accessibilityLabel="Primary navigation"
      style={[
        styles.bottomNavigation,
        { backgroundColor: palette.surface, borderColor: palette.border },
      ]}
    >
      {entries.map((entry) => (
        <ShellNavigationItem
          active={active?.id === entry.id}
          compact
          entry={entry}
          key={entry.id}
        />
      ))}
    </View>
  );
}

function ShellNavigationItem({
  active,
  compact = false,
  entry,
}: {
  readonly active: boolean;
  readonly compact?: boolean;
  readonly entry: ScreenCatalogEntry;
}) {
  const palette = useAppPalette();
  return (
    <Link href={entry.path} asChild>
      <Pressable
        accessibilityRole="tab"
        accessibilityState={{ selected: active }}
        style={compact ? styles.compactNavigationPressable : styles.navigationPressable}
      >
        <View
          style={[
            styles.navigationItem,
            compact ? styles.compactNavigationItem : null,
            {
              backgroundColor: active ? palette.selected : "transparent",
              borderColor: active ? palette.focus : "transparent",
            },
          ]}
        >
          <Text
            numberOfLines={compact ? 1 : 2}
            style={[
              styles.navigationLabel,
              compact ? styles.compactNavigationLabel : null,
              { color: active ? palette.selectedText : palette.text },
            ]}
          >
            {entry.label}
          </Text>
        </View>
      </Pressable>
    </Link>
  );
}

function ContextPanel({ active }: { readonly active: ScreenCatalogEntry | undefined }) {
  const palette = useAppPalette();
  return (
    <View
      accessibilityLabel="Workspace context"
      style={[
        styles.contextPanel,
        { backgroundColor: palette.surface, borderColor: palette.border },
      ]}
    >
      <Text accessibilityRole="header" style={[styles.contextTitle, { color: palette.text }]}>
        Workspace
      </Text>
      <Text style={[styles.contextLabel, { color: palette.textMuted }]}>Current area</Text>
      <Text style={[styles.contextValue, { color: palette.text }]}>
        {active?.label ?? "Detail"}
      </Text>
      {active?.section === "nodes" ? (
        <>
          <Text style={[styles.contextLabel, { color: palette.textMuted }]}>Nodes</Text>
          <Text style={[styles.contextValue, { color: palette.text }]}>Live status</Text>
          <Text style={[styles.contextHint, { color: palette.textMuted }]}>
            Paired nodes appear here after pairing is complete.
          </Text>
        </>
      ) : null}
    </View>
  );
}

export function ShellSurface({ children }: PropsWithChildren) {
  return <View style={styles.content}>{children}</View>;
}

const styles = StyleSheet.create({
  safeArea: { flex: 1 },
  header: {
    alignItems: "center",
    borderBottomWidth: 1,
    flexDirection: "row",
    gap: space.md,
    justifyContent: "space-between",
    minHeight: 58,
    paddingHorizontal: space.md,
    paddingVertical: space.sm,
  },
  brand: { fontSize: 24, fontWeight: "800", letterSpacing: -0.5, lineHeight: 30 },
  shellBody: { flex: 1, flexDirection: "row", minHeight: 0 },
  content: { flex: 1, minWidth: 0 },
  rail: {
    borderRightWidth: 1,
    gap: space.xs,
    padding: space.sm,
    width: 224,
  },
  bottomNavigation: {
    borderTopWidth: 1,
    flexDirection: "row",
    minHeight: 66,
    paddingHorizontal: space.xs,
    paddingVertical: space.xs,
  },
  navigationPressable: { borderRadius: radius.sm, minHeight: 48, width: "100%" },
  compactNavigationPressable: { borderRadius: radius.sm, flex: 1, minHeight: 48, minWidth: 0 },
  navigationItem: {
    borderRadius: radius.sm,
    borderWidth: 2,
    justifyContent: "center",
    minHeight: 48,
    paddingHorizontal: 12,
    paddingVertical: 9,
  },
  compactNavigationItem: { alignItems: "center", flex: 1, minWidth: 0, paddingHorizontal: 4 },
  navigationLabel: { fontSize: 15, fontWeight: "600", lineHeight: 20 },
  compactNavigationLabel: { fontSize: 12, lineHeight: 16, textAlign: "center" },
  contextPanel: {
    borderLeftWidth: 1,
    gap: space.sm,
    padding: space.md,
    width: 280,
  },
  contextTitle: { fontSize: 20, fontWeight: "700", lineHeight: 27 },
  contextLabel: { fontSize: 12, fontWeight: "700", lineHeight: 17, textTransform: "uppercase" },
  contextValue: { fontSize: 16, fontWeight: "600", lineHeight: 23 },
  contextHint: { fontSize: 13, lineHeight: 19 },
});
