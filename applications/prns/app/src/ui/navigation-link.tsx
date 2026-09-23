import { type Href, Link } from "expo-router";
import { type PropsWithChildren, useState } from "react";
import { Pressable, StyleSheet, Text } from "react-native";

import { radius, space, useAppPalette } from "./theme";

export function NavigationLink({
  accessibilityLabel,
  children,
  direction = "forward",
  href,
}: PropsWithChildren<{
  readonly href: Href;
  readonly accessibilityLabel?: string;
  readonly direction?: "forward" | "back";
}>) {
  const palette = useAppPalette();
  const [pressed, setPressed] = useState(false);
  const [focused, setFocused] = useState(false);
  const arrow = (
    <Text aria-hidden style={[styles.arrow, { color: palette.textMuted }]}>
      {direction === "back" ? "←" : "→"}
    </Text>
  );
  return (
    <Link href={href} asChild>
      <Pressable
        accessibilityRole="link"
        {...(accessibilityLabel === undefined ? {} : { accessibilityLabel })}
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
        {direction === "back" ? arrow : null}
        <Text style={[styles.label, { color: palette.text }]}>{children}</Text>
        {direction === "forward" ? arrow : null}
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
    paddingHorizontal: 12,
    paddingVertical: 8,
  },
  label: { flex: 1, flexShrink: 1, fontSize: 16, fontWeight: "600", lineHeight: 22, minWidth: 0 },
  arrow: { flexShrink: 0, fontSize: 20, lineHeight: 24 },
});
