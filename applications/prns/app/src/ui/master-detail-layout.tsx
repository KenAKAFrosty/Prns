import { Slot, usePathname } from "expo-router";
import type { ReactNode } from "react";
import { StyleSheet, Text, useWindowDimensions, View } from "react-native";

import { layoutModeForWidth, space, useAppPalette } from "./theme";

type MasterDetailLayoutProps = {
  readonly emptyDescription: string;
  readonly emptyTitle: string;
  readonly master: ReactNode;
  readonly rootPath: string;
  readonly sectionLabel: string;
};

export function MasterDetailLayout({
  emptyDescription,
  emptyTitle,
  master,
  rootPath,
  sectionLabel,
}: MasterDetailLayoutProps) {
  const { width } = useWindowDimensions();
  const pathname = usePathname();
  const palette = useAppPalette();
  const isRoot = pathname === rootPath;
  const isFamilyRoute = isRoot || pathname.startsWith(`${rootPath}/`);
  const isWide = layoutModeForWidth(width) === "wide" && isFamilyRoute;

  return (
    <View
      style={[
        styles.container,
        isWide ? styles.workspace : null,
        isWide && !isRoot ? styles.detailFirst : null,
      ]}
    >
      <View
        accessibilityLabel={
          isWide
            ? isRoot
              ? `${sectionLabel} list`
              : `${sectionLabel} detail`
            : `${sectionLabel} content`
        }
        role="region"
        style={[
          isWide && isRoot ? styles.masterPane : styles.routePane,
          isWide && isRoot
            ? { backgroundColor: palette.background, borderColor: palette.border }
            : null,
        ]}
      >
        <Slot />
      </View>
      {isWide ? (
        <View
          accessibilityLabel={isRoot ? `${sectionLabel} detail` : `${sectionLabel} list`}
          role="region"
          style={[
            isRoot ? styles.detailPane : styles.masterPane,
            { backgroundColor: palette.background, borderColor: palette.border },
          ]}
        >
          {isRoot ? (
            <View style={styles.emptyDetail}>
              <Text accessibilityRole="header" style={[styles.emptyTitle, { color: palette.text }]}>
                {emptyTitle}
              </Text>
              <Text style={[styles.emptyDescription, { color: palette.textMuted }]}>
                {emptyDescription}
              </Text>
            </View>
          ) : (
            master
          )}
        </View>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  container: { flex: 1, minHeight: 0, minWidth: 0 },
  workspace: { flexDirection: "row" },
  detailFirst: { flexDirection: "row-reverse" },
  routePane: { flex: 1, minHeight: 0, minWidth: 0 },
  masterPane: {
    borderRightWidth: 1,
    flexBasis: 360,
    flexGrow: 0,
    flexShrink: 0,
    minHeight: 0,
    minWidth: 320,
  },
  detailPane: { flex: 1, minHeight: 0, minWidth: 0 },
  emptyDetail: {
    alignItems: "center",
    flex: 1,
    gap: space.sm,
    justifyContent: "center",
    padding: space.xl,
  },
  emptyTitle: { fontSize: 24, fontWeight: "700", lineHeight: 31, textAlign: "center" },
  emptyDescription: { fontSize: 16, lineHeight: 24, maxWidth: 440, textAlign: "center" },
});
