import { Link, type Href } from "expo-router";
import type { PropsWithChildren } from "react";
import { Pressable, StyleSheet, Text } from "react-native";

import { radius, space, useAppPalette } from "./theme";

export function NavigationLink({ children, href }: PropsWithChildren<{ readonly href: Href }>) {
  const palette = useAppPalette();
  return (
    <Link href={href} asChild>
      <Pressable
        accessibilityRole="link"
        style={({ pressed }) => [
          styles.link,
          {
            backgroundColor: pressed ? palette.selected : palette.surface,
            borderColor: palette.border,
          },
        ]}
      >
        <Text style={[styles.label, { color: palette.text }]}>{children}</Text>
        <Text aria-hidden style={[styles.arrow, { color: palette.textMuted }]}>
          →
        </Text>
      </Pressable>
    </Link>
  );
}

const styles = StyleSheet.create({
  link: {
    alignItems: "center",
    borderRadius: radius.sm,
    borderWidth: 1,
    flexDirection: "row",
    gap: space.md,
    justifyContent: "space-between",
    minHeight: 48,
    paddingHorizontal: space.md,
    paddingVertical: 10,
  },
  label: { flex: 1, fontSize: 16, fontWeight: "600", lineHeight: 22 },
  arrow: { fontSize: 20, lineHeight: 24 },
});
