import { Children, type PropsWithChildren, type ReactNode, useState } from "react";
import {
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  useWindowDimensions,
  View,
  type PressableProps,
  type StyleProp,
  type ViewStyle,
} from "react-native";

import { radius, space, useAppPalette } from "./theme";
import { AndroidKeyboardScreen } from "./android-keyboard-screen";

export function Screen({ children }: PropsWithChildren) {
  const palette = useAppPalette();
  if (Platform.OS === "android") {
    return (
      <AndroidKeyboardScreen
        style={[styles.screen, { backgroundColor: palette.background }]}
        contentContainerStyle={styles.screenContent}
      >
        {children}
      </AndroidKeyboardScreen>
    );
  }
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

/** A title and its status/actions share space, but wrap rather than truncate. */
export function CardHeader({ title, children }: PropsWithChildren<{ readonly title: string }>) {
  const palette = useAppPalette();
  return (
    <View style={styles.cardHeader}>
      <Text
        accessibilityRole="header"
        aria-level={2}
        style={[styles.subheading, styles.cardHeaderTitle, { color: palette.text }]}
      >
        {title}
      </Text>
      {children === undefined ? null : <View style={styles.headerActions}>{children}</View>}
    </View>
  );
}

/** Separate related groups inside a card without nesting another card. */
export function CardSection({ title, children }: PropsWithChildren<{ readonly title?: string }>) {
  const palette = useAppPalette();
  return (
    <View style={[styles.cardSection, { borderTopColor: palette.border }]}>
      {title === undefined ? null : <Subheading>{title}</Subheading>}
      {children}
    </View>
  );
}

/** Related controls share a row when there is room; large text stays full width. */
export function ActionRow({ children }: PropsWithChildren) {
  const { fontScale } = useWindowDimensions();
  return (
    <View style={styles.actionRow}>
      {Children.map(children, (child) =>
        child === null ? null : (
          <View style={[styles.actionItem, fontScale >= 1.4 ? styles.actionItemStacked : null]}>
            {child}
          </View>
        ),
      )}
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
  const { width, fontScale } = useWindowDimensions();
  const stacked = width / fontScale < 360 || value.length > 28 || value.includes("\n");
  return (
    <View style={[styles.keyValue, stacked ? styles.keyValueStacked : null]}>
      <Text style={[styles.key, stacked ? styles.stackedText : null, { color: palette.textMuted }]}>
        {label}
      </Text>
      <Text
        selectable
        style={[styles.value, stacked ? styles.stackedText : null, { color: palette.text }]}
      >
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
    gap: 12,
    maxWidth: 880,
    padding: space.md,
    paddingBottom: space.xl,
    width: "100%",
  },
  heading: { fontSize: 26, fontWeight: "700", lineHeight: 32 },
  subheading: { fontSize: 18, fontWeight: "600", lineHeight: 24 },
  body: { fontSize: 16, lineHeight: 22 },
  badge: {
    alignSelf: "flex-start",
    maxWidth: "100%",
    borderRadius: radius.pill,
    paddingHorizontal: 8,
    paddingVertical: 3,
  },
  badgeText: { fontSize: 13, fontWeight: "700", lineHeight: 18 },
  card: {
    borderRadius: radius.md,
    borderWidth: 1,
    gap: space.sm,
    padding: 12,
    minWidth: 0,
  },
  cardHeader: {
    alignItems: "center",
    flexDirection: "row",
    flexWrap: "wrap",
    gap: space.sm,
  },
  cardHeaderTitle: { flexBasis: 120, flexGrow: 1, flexShrink: 1, minWidth: 0 },
  headerActions: { flexDirection: "row", flexWrap: "wrap", gap: space.sm, maxWidth: "100%" },
  cardSection: {
    borderTopWidth: StyleSheet.hairlineWidth,
    gap: space.sm,
    marginTop: 4,
    paddingTop: 12,
  },
  actionRow: { flexDirection: "row", flexWrap: "wrap", gap: space.sm, minWidth: 0 },
  actionItem: { flexBasis: 144, flexGrow: 1, minWidth: 0 },
  actionItemStacked: { flexBasis: "100%" },
  button: {
    alignItems: "center",
    borderRadius: radius.sm,
    borderWidth: 2,
    justifyContent: "center",
    minHeight: 48,
    paddingHorizontal: 12,
    paddingVertical: 8,
  },
  buttonText: { fontSize: 16, fontWeight: "700", lineHeight: 22, textAlign: "center" },
  keyValue: { alignItems: "flex-start", flexDirection: "row", gap: 12, paddingVertical: 3 },
  keyValueStacked: { flexDirection: "column", gap: space.xs },
  key: { flex: 1, minWidth: 0, fontSize: 13, fontWeight: "600", lineHeight: 20 },
  value: { flex: 1.3, minWidth: 0, fontSize: 15, lineHeight: 20, textAlign: "right" },
  stackedText: { flex: 0, textAlign: "left", alignSelf: "stretch" },
  stack: { gap: 12 },
});
