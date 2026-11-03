import { forwardRef, useContext, useEffect, useImperativeHandle, useRef, useState } from "react";
import type { PropsWithChildren, ReactNode } from "react";
import {
  Modal, Platform, Pressable, ScrollView, StyleSheet, Text as NativeText,
  TextInput as NativeTextInput, View,
} from "react-native";
import type { ButtonProps as NativeButtonProps, PressableProps, StyleProp, TextProps, TextInputInstance, TextInputProps, ViewInstance, ViewProps, ViewStyle } from "react-native";
import Svg, { Path } from "react-native-svg";
import { AndroidKeyboardScreen } from "./android-keyboard-screen";
import { IOSKeyboardScreen } from "./ios-keyboard-screen";
import { FieldKeyboardContext } from "./field-keyboard";
import { useAppPalette } from "./theme";
import { InfoDisclosure } from "./InfoDisclosure";
import { ActionIcon } from "./action-icon";
import type { ActionIconName } from "./action-icon";
import { SafeAreaView } from "react-native-safe-area-context";

export function Text({ style, ...props }: TextProps) {
  const palette = useAppPalette();
  return <NativeText {...props} style={[base.text, { color: palette.text }, style]} />;
}

export const TextInput = forwardRef<TextInputInstance, TextInputProps>(function TextInput({ style, onFocus, onBlur, onContentSizeChange, ...props }, ref) {
  const palette = useAppPalette();
  const keyboard = useContext(FieldKeyboardContext);
  const field = useRef<TextInputInstance>(null);
  useImperativeHandle(ref, () => field.current!);
  return <NativeTextInput {...props} ref={field} placeholderTextColor={palette.textMuted}
    onFocus={(event) => { keyboard?.focus(field.current); onFocus?.(event); }}
    onBlur={(event) => { keyboard?.blur(field.current); onBlur?.(event); }}
    onContentSizeChange={(event) => { keyboard?.reveal(field.current); onContentSizeChange?.(event); }}
    selectionColor={palette.accent} style={[base.input, { color: palette.text, backgroundColor: palette.surface, borderColor: palette.border },
      props.multiline && keyboard?.maximumInputHeight !== undefined ? { maxHeight: keyboard.maximumInputHeight } : undefined, style]} />;
});

export type ButtonProps = Omit<NativeButtonProps, "style"> & {
  tone?: "primary" | "secondary" | "destructive" | "quiet";
  size?: "normal" | "compact";
  fill?: boolean;
  icon?: ActionIconName;
  style?: StyleProp<ViewStyle>;
};

export function Button({ title, disabled, color, accessibilityState, tone = "secondary", size = "normal", fill = false, icon, style, ...props }: ButtonProps) {
  const palette = useAppPalette();
  const [focused, setFocused] = useState(false);
  const selected = accessibilityState?.selected === true;
  const background = selected ? palette.selected : tone === "primary" ? palette.accent : tone === "destructive" ? palette.destructive : tone === "quiet" ? "transparent" : palette.surfaceRaised;
  const foreground = color ?? (selected ? palette.selectedText : tone === "primary" ? palette.accentText : tone === "destructive" ? palette.destructiveText : tone === "quiet" ? palette.accent : palette.text);
  return <Pressable {...props} accessibilityRole="button" disabled={disabled}
    accessibilityState={{ ...accessibilityState, disabled: disabled ?? false }}
    onFocus={() => setFocused(true)} onBlur={() => setFocused(false)}
    style={({ pressed }) => [base.button, size === "compact" && base.compactButton, fill && base.fill, {
      borderColor: focused ? palette.focus : "transparent", backgroundColor: pressed && tone === "quiet" ? palette.surfaceRaised : background,
      opacity: disabled ? 0.48 : pressed ? 0.82 : 1,
    }, style]}>
    {icon !== undefined && <ActionIcon name={icon} color={foreground} />}
    <NativeText style={[base.buttonLabel, { color: foreground }]}>{title}</NativeText>
  </Pressable>;
}

export function Actions({ style, ...props }: ViewProps) {
  return <View {...props} style={[base.actions, style]} />;
}

export function IconButton({ label, icon, onPress, disabled = false, expanded }: {
  label: string; icon: ActionIconName; onPress: () => void; disabled?: boolean; expanded?: boolean;
}) {
  const palette = useAppPalette();
  const [focused, setFocused] = useState(false);
  return <Pressable accessibilityRole="button" accessibilityLabel={label}
    accessibilityState={{ disabled, ...(expanded === undefined ? {} : { expanded }) }} disabled={disabled} onPress={onPress}
    onFocus={() => setFocused(true)} onBlur={() => setFocused(false)}
    style={({ pressed }) => [base.iconButton, { borderColor: focused ? palette.focus : "transparent", backgroundColor: pressed ? palette.surfaceRaised : "transparent", opacity: disabled ? 0.48 : 1 }]}>
    <ActionIcon name={icon} color={palette.textMuted} />
  </Pressable>;
}

export type MenuItem = { title: string; onPress: () => void; destructive?: boolean; disabled?: boolean };
export function ActionMenu({ label, items, disabled = false }: { label: string; items: readonly MenuItem[]; disabled?: boolean }) {
  const palette = useAppPalette();
  const [open, setOpen] = useState(false);
  const pending = useRef<(() => void) | undefined>(undefined);
  const choosing = useRef(false);
  const unavailable = useRef(disabled);
  unavailable.current = disabled;
  const close = () => { pending.current = undefined; setOpen(false); };
  const dismissed = () => { const action = pending.current; pending.current = undefined; if (!unavailable.current) action?.(); };
  const choose = (item: MenuItem) => {
    if (disabled || item.disabled || choosing.current) return;
    choosing.current = true;
    pending.current = item.onPress;
    setOpen(false);
    // iOS must finish dismissing this native modal before an action presents
    // another native controller, such as the contact-removal confirmation.
    if (Platform.OS !== "ios") dismissed();
  };
  useEffect(() => { if (disabled) { pending.current = undefined; setOpen(false); } }, [disabled]);
  useEffect(() => () => { pending.current = undefined; }, []);
  return <>
    <IconButton label={label} icon="more" expanded={open} disabled={disabled} onPress={() => { if (pending.current !== undefined) return; choosing.current = false; setOpen(true); }} />
    <Modal visible={open && !disabled} transparent animationType="fade" onRequestClose={close} onDismiss={dismissed}>
      <View style={base.scrim}>
        <Pressable accessible={false} onPress={close} style={StyleSheet.absoluteFill} />
        <SafeAreaView edges={["bottom", "left", "right"]} accessibilityViewIsModal onAccessibilityEscape={close}
          style={[base.menu, { backgroundColor: palette.surface }]}>
          <View style={base.menuHeader}><Text accessibilityRole="header" style={base.menuTitle}>{label}</Text>
            <IconButton label={`Close ${label}`} icon="close" onPress={close} /></View>
          <ScrollView keyboardShouldPersistTaps="handled" contentContainerStyle={base.menuItems}>
            {items.map((item) => <Pressable key={item.title} accessibilityRole="button" accessibilityLabel={item.title}
              accessibilityState={{ disabled: disabled || (item.disabled ?? false) }} disabled={disabled || item.disabled} onPress={() => choose(item)}
              style={({ pressed }) => [base.menuItem, { backgroundColor: pressed ? palette.surfaceRaised : "transparent", opacity: item.disabled ? 0.48 : 1 }]}>
              <Text style={{ color: item.destructive ? palette.destructive : palette.text }}>{item.title}</Text>
            </Pressable>)}
          </ScrollView>
        </SafeAreaView>
      </View>
    </Modal>
  </>;
}

export function PageHeader({ title, subtitle, help, trailing, children }: PropsWithChildren<{
  title: string; subtitle?: string | undefined; help?: { label: string; content: ReactNode } | undefined; trailing?: ReactNode;
}>) {
  const palette = useAppPalette();
  return <View style={base.pageHeader}>
    <View style={base.headerLine}>
      <View style={base.pageTitle}><Heading help={help}>{title}</Heading></View>
      {trailing !== undefined && <View style={base.headerTrailing}>{trailing}</View>}
    </View>
    {subtitle !== undefined && <Text style={{ fontSize: 14, lineHeight: 20, color: palette.textMuted }}>{subtitle}</Text>}
    {children !== undefined && <Actions style={base.headerActions}>{children}</Actions>}
  </View>;
}

export function Badge({ children, warning = false }: PropsWithChildren<{ warning?: boolean }>) {
  const palette = useAppPalette();
  return <View style={[base.badge, { backgroundColor: warning ? palette.warningSurface : palette.surfaceRaised }]}>
    <Text style={{ fontSize: 12, lineHeight: 18, fontWeight: "600", color: warning ? palette.warning : palette.textMuted }}>{children}</Text>
  </View>;
}

export type ListRowProps = Omit<PressableProps, "children" | "style"> & {
  title: string; subtitle?: string | undefined; badge?: ReactNode; style?: StyleProp<ViewStyle>;
};
export const ListRow = forwardRef<ViewInstance, ListRowProps>(function ListRow({ title, subtitle, badge, accessibilityLabel, accessibilityRole = "button",
  accessibilityState, disabled = false, onFocus, onBlur, style, ...props }, ref) {
  const palette = useAppPalette();
  const [focused, setFocused] = useState(false);
  return <Pressable {...props} ref={ref} accessibilityRole={accessibilityRole} accessibilityLabel={accessibilityLabel ?? title}
    accessibilityState={{ ...accessibilityState, disabled: disabled ?? false }} disabled={disabled}
    onFocus={(event) => { setFocused(true); onFocus?.(event); }} onBlur={(event) => { setFocused(false); onBlur?.(event); }}
    style={({ pressed }) => [base.listRow, { backgroundColor: pressed ? palette.surfaceRaised : palette.surface, borderColor: focused ? palette.focus : palette.border, opacity: disabled ? 0.48 : 1 }, style]}>
    <View style={{ flex: 1, minWidth: 0, gap: 6 }}>
      <Text style={{ fontSize: 17, lineHeight: 23, fontWeight: "600" }}>{title}</Text>
      {subtitle !== undefined && <Text style={{ fontSize: 14, lineHeight: 20, color: palette.textMuted }}>{subtitle}</Text>}
      {badge}
    </View>
    <Svg width={20} height={20} viewBox="0 0 24 24" accessibilityElementsHidden importantForAccessibility="no">
      <Path d="M9 6l6 6-6 6" fill="none" stroke={palette.textMuted} strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" />
    </Svg>
  </Pressable>;
});

export function Panel({ style, ...props }: ViewProps) {
  const palette = useAppPalette();
  return <View {...props} style={[base.panel, { backgroundColor: palette.surface, borderColor: palette.border }, style]} />;
}

export function Screen({ children }: PropsWithChildren) {
  const palette = useAppPalette();
  const props = { style: { flex: 1, backgroundColor: palette.background }, contentContainerStyle: base.page };
  return Platform.OS === "android"
    ? <AndroidKeyboardScreen {...props}>{children}</AndroidKeyboardScreen>
    : Platform.OS === "ios" ? <IOSKeyboardScreen {...props}>{children}</IOSKeyboardScreen>
      : <ScrollView {...props} keyboardDismissMode="interactive" keyboardShouldPersistTaps="handled">{children}</ScrollView>;
}

export function Heading({ children, help }: PropsWithChildren<{ readonly help?: { readonly label: string; readonly content: ReactNode } | undefined }>) {
  if (help === undefined) return <Text accessibilityRole="header" style={base.heading}>{children}</Text>;
  return <View style={base.headingRow}>
    <Text accessibilityRole="header" style={[base.heading, { flex: 1 }]}>{children}</Text>
    <InfoDisclosure label={help.label}>{help.content}</InfoDisclosure>
  </View>;
}

const base = StyleSheet.create({
  text: { fontSize: 16, lineHeight: 23 },
  heading: { fontSize: 26, lineHeight: 32, fontWeight: "700" },
  headingRow: { flexDirection: "row", alignItems: "center", gap: 12 },
  button: { alignSelf: "flex-start", maxWidth: "100%", minHeight: 44, borderWidth: 1, borderRadius: 12, paddingHorizontal: 14, paddingVertical: 9, flexDirection: "row", gap: 8, alignItems: "center", justifyContent: "center" },
  fill: { alignSelf: "stretch", width: "100%" },
  actions: { flexDirection: "row", alignItems: "center", flexWrap: "wrap", gap: 8 },
  iconButton: { minWidth: 44, minHeight: 44, flexShrink: 0, borderWidth: 2, borderRadius: 12, alignItems: "center", justifyContent: "center" },
  pageHeader: { alignItems: "stretch", gap: 12 },
  headerLine: { flexDirection: "row", alignItems: "center", gap: 8 },
  pageTitle: { flex: 1, minWidth: 0 },
  headerTrailing: { flexShrink: 0 },
  headerActions: { width: "100%", flexShrink: 0 },
  scrim: { flex: 1, backgroundColor: "#0008", justifyContent: "flex-end" },
  menu: { maxHeight: "80%", borderTopLeftRadius: 20, borderTopRightRadius: 20 },
  menuHeader: { flexDirection: "row", alignItems: "center", paddingLeft: 20, paddingRight: 8, paddingTop: 8 },
  menuTitle: { flex: 1, fontSize: 19, lineHeight: 26, fontWeight: "600" },
  menuItems: { paddingHorizontal: 8, paddingBottom: 12 },
  menuItem: { minHeight: 48, paddingVertical: 12, paddingHorizontal: 12, borderRadius: 10, justifyContent: "center" },
  compactButton: { minHeight: 44, paddingHorizontal: 10, paddingVertical: 8 },
  badge: { alignSelf: "flex-start", paddingHorizontal: 8, paddingVertical: 3, borderRadius: 8 },
  listRow: { minHeight: 60, padding: 14, borderWidth: StyleSheet.hairlineWidth, borderRadius: 14, gap: 12, flexDirection: "row", alignItems: "center" },
  buttonLabel: { flexShrink: 1, fontSize: 15, lineHeight: 22, fontWeight: "600", textAlign: "center" },
  input: { minHeight: 48, borderWidth: 1, borderRadius: 10, padding: 13, fontSize: 16 },
  panel: { borderWidth: StyleSheet.hairlineWidth, borderRadius: 16, padding: 16, gap: 12 },
  page: { padding: 16, paddingBottom: 32, gap: 20, width: "100%", maxWidth: 1040, alignSelf: "center" },
});
