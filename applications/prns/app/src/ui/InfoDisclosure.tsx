import { useState } from "react";
import type { PropsWithChildren } from "react";
import { Modal, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";
import Svg, { Circle, Path } from "react-native-svg";
import { useAppPalette } from "./theme";

/** Optional context. Action results and recovery instructions stay on the screen. */
export function InfoDisclosure({ label, children }: PropsWithChildren<{ readonly label: string }>) {
  const [open, setOpen] = useState(false);
  const [focused, setFocused] = useState(false);
  const palette = useAppPalette();
  const close = () => setOpen(false);
  return <>
    <Pressable accessibilityRole="button" accessibilityLabel={label} accessibilityState={{ expanded: open }}
      onPress={() => setOpen(true)} onFocus={() => setFocused(true)} onBlur={() => setFocused(false)}
      style={({ pressed }) => [styles.trigger, { borderColor: focused ? palette.focus : "transparent", backgroundColor: pressed ? palette.surfaceRaised : "transparent" }]}>
      <Svg width={22} height={22} viewBox="0 0 24 24" accessibilityElementsHidden importantForAccessibility="no">
        <Circle cx={12} cy={12} r={9} fill="none" stroke={palette.textMuted} strokeWidth={1.7} />
        <Path d="M12 11v6 M12 7v.5" fill="none" stroke={palette.textMuted} strokeWidth={1.9} strokeLinecap="round" />
      </Svg>
    </Pressable>
    <Modal visible={open} transparent animationType="fade" onRequestClose={close}>
      <View style={styles.overlay}>
        <Pressable accessible={false} onPress={close} style={StyleSheet.absoluteFill} />
        <SafeAreaView edges={["bottom", "left", "right"]} accessibilityViewIsModal onAccessibilityEscape={close}
          style={[styles.sheet, { backgroundColor: palette.surface }]}>
          <View style={styles.header}>
            <Text accessibilityRole="header" style={[styles.title, { color: palette.text }]}>{label}</Text>
            <Pressable accessibilityRole="button" accessibilityLabel={`Close ${label}`} onPress={close} style={styles.trigger}>
              <Text accessible={false} style={[styles.close, { color: palette.textMuted }]}>×</Text>
            </Pressable>
          </View>
          <ScrollView contentContainerStyle={styles.content} keyboardShouldPersistTaps="handled">{children}</ScrollView>
        </SafeAreaView>
      </View>
    </Modal>
  </>;
}

const styles = StyleSheet.create({
  trigger: { minWidth: 44, minHeight: 44, flexShrink: 0, borderWidth: 2, borderRadius: 22, alignItems: "center", justifyContent: "center" },
  overlay: { flex: 1, justifyContent: "flex-end", backgroundColor: "#0008" },
  sheet: { maxHeight: "80%", borderTopLeftRadius: 18, borderTopRightRadius: 18 },
  header: { flexDirection: "row", alignItems: "center", paddingLeft: 20, paddingRight: 8, paddingTop: 8, gap: 8 },
  title: { flex: 1, fontSize: 20, lineHeight: 28, fontWeight: "600" },
  close: { fontSize: 28, lineHeight: 32 },
  content: { paddingHorizontal: 20, paddingBottom: 24, gap: 16 },
});
