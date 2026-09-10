import { type Href, Link } from "expo-router";
import { type PropsWithChildren, useState } from "react";
import { Pressable, StyleSheet, Text } from "react-native";

import { radius, space, useAppPalette } from "./theme";

export function NavigationLink({ children, href }: PropsWithChildren<{ readonly href: Href }>) {
  const palette = useAppPalette();
  const [pressed, setPressed] = useState(false);
  const [focused, setFocused] = useState(false);
  return (
    <Link href={href} asChild>
      <Pressable
        accessibilityRole="link"
        onBlur={() => setFocused(false)}
        onFocus={() => setFocused(true)}
        onPressIn={() => setPressed(true)}
        onPressOut={() => setPressed(false)}
        // Expo Router's asChild slot merges style objects, not Pressable callbacks.
        style={StyleSheet.flatten([
          styles.link,
          {
            backgroundColor: pressed ? palette.selected : palette.surface,
            borderColor: focused ? palette.focus : palette.border,
          },
        ])}
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
    minWidth: 0,
    paddingHorizontal: space.md,
    paddingVertical: 10,
  },
  label: { flex: 1, flexShrink: 1, fontSize: 16, fontWeight: "600", lineHeight: 22, minWidth: 0 },
  arrow: { flexShrink: 0, fontSize: 20, lineHeight: 24 },
});
