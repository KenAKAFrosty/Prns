import { type PropsWithChildren, type ReactNode, useState } from "react";
import {
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
  type PressableProps,
  type StyleProp,
  type ViewStyle,
} from "react-native";

import { radius, space, useAppPalette } from "./theme";

export function Screen({ children }: PropsWithChildren) {
  const palette = useAppPalette();
  return (
    <ScrollView
      style={[styles.screen, { backgroundColor: palette.background }]}
      contentContainerStyle={styles.screenContent}
      automaticallyAdjustKeyboardInsets
      keyboardDismissMode={Platform.OS === "ios" ? "interactive" : "on-drag"}
      keyboardShouldPersistTaps="handled"
    >
      {children}
    </ScrollView>
  );
}

export function ScreenHeading({ children }: PropsWithChildren) {
  const palette = useAppPalette();
  return (
    <Text accessibilityRole="header" style={[styles.heading, { color: palette.text }]}>
      {children}
    </Text>
  );
}

export function Subheading({ children }: PropsWithChildren) {
  const palette = useAppPalette();
  return (
    <Text
      accessibilityRole="header"
      aria-level={2}
      style={[styles.subheading, { color: palette.text }]}
    >
      {children}
    </Text>
  );
}

export function BodyText({
  children,
  muted = false,
}: PropsWithChildren<{ readonly muted?: boolean }>) {
  const palette = useAppPalette();
  return (
    <Text style={[styles.body, { color: muted ? palette.textMuted : palette.text }]}>
      {children}
    </Text>
  );
}

export function Badge({
  children,
  tone = "neutral",
}: PropsWithChildren<{ readonly tone?: "neutral" | "warning" }>) {
  const palette = useAppPalette();
  const warning = tone === "warning";
  return (
    <View
      style={[
        styles.badge,
        { backgroundColor: warning ? palette.warningSurface : palette.surfaceRaised },
      ]}
    >
      <Text style={[styles.badgeText, { color: warning ? palette.warning : palette.textMuted }]}>
        {children}
      </Text>
    </View>
  );
}

export function Card({
  children,
  style,
}: PropsWithChildren<{ readonly style?: StyleProp<ViewStyle> }>) {
  const palette = useAppPalette();
  return (
    <View
      style={[
        styles.card,
        { backgroundColor: palette.surface, borderColor: palette.border },
        style,
      ]}
    >
      {children}
    </View>
  );
}

type ButtonProps = Pick<
  PressableProps,
  "accessibilityLabel" | "accessibilityState" | "disabled" | "onPress"
> & {
  readonly children: ReactNode;
  readonly tone?: "primary" | "secondary" | "destructive";
};

export function Button({
  accessibilityLabel,
  accessibilityState,
  children,
  disabled = false,
  onPress,
  tone = "primary",
}: ButtonProps) {
  const palette = useAppPalette();
  const [focused, setFocused] = useState(false);
  const isDisabled = disabled === true;
  const primary = tone === "primary";
  const destructive = tone === "destructive";
  const backgroundColor = primary
    ? palette.accent
    : destructive
      ? palette.destructive
      : palette.surface;
  const color = primary || destructive ? palette.accentText : palette.text;
  const labelled = accessibilityLabel === undefined ? {} : { accessibilityLabel };

  return (
    <Pressable
      {...labelled}
      accessibilityRole="button"
      accessibilityState={{ ...accessibilityState, disabled: isDisabled }}
      disabled={isDisabled}
      onBlur={() => setFocused(false)}
      onFocus={() => setFocused(true)}
      onPress={onPress}
      style={({ pressed }) => [
        styles.button,
        {
          backgroundColor,
          borderColor: focused
            ? palette.focus
            : primary || destructive
              ? backgroundColor
              : palette.border,
          opacity: isDisabled ? 0.5 : pressed ? 0.8 : 1,
        },
      ]}
    >
      <Text style={[styles.buttonText, { color }]}>{children}</Text>
    </Pressable>
  );
}

export function KeyValue({ label, value }: { readonly label: string; readonly value: string }) {
  const palette = useAppPalette();
  return (
    <View style={styles.keyValue}>
      <Text style={[styles.key, { color: palette.textMuted }]}>{label}</Text>
      <Text selectable style={[styles.value, { color: palette.text }]}>
        {value}
      </Text>
    </View>
  );
}

export function CardStack({ children }: PropsWithChildren) {
  return <View style={styles.stack}>{children}</View>;
}

const styles = StyleSheet.create({
  screen: { flex: 1, minWidth: 0 },
  screenContent: {
    alignSelf: "center",
    gap: space.md,
    maxWidth: 880,
    padding: space.lg,
    paddingBottom: space.xl,
    width: "100%",
  },
  heading: { fontSize: 32, fontWeight: "700", lineHeight: 39 },
  subheading: { fontSize: 21, fontWeight: "600", lineHeight: 28 },
  body: { fontSize: 16, lineHeight: 24 },
  badge: {
    alignSelf: "flex-start",
    maxWidth: "100%",
    borderRadius: radius.pill,
    paddingHorizontal: 10,
    paddingVertical: 5,
  },
  badgeText: { fontSize: 13, fontWeight: "700", lineHeight: 18 },
  card: {
    borderRadius: radius.md,
    borderWidth: 1,
    gap: space.sm,
    padding: space.md,
    minWidth: 0,
  },
  button: {
    alignItems: "center",
    borderRadius: radius.sm,
    borderWidth: 2,
    justifyContent: "center",
    minHeight: 48,
    paddingHorizontal: space.md,
    paddingVertical: 10,
  },
  buttonText: { fontSize: 16, fontWeight: "700", lineHeight: 22, textAlign: "center" },
  keyValue: { gap: space.xs },
  key: { fontSize: 13, fontWeight: "600", lineHeight: 18 },
  value: { fontSize: 16, lineHeight: 24 },
  stack: { gap: space.md },
});
