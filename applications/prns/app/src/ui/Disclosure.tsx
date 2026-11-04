import { useState } from "react";
import type { PropsWithChildren } from "react";
import { Pressable, View } from "react-native";
import Svg, { Path } from "react-native-svg";
import { Text } from "./primitives";
import { useAppPalette } from "./theme";

/** An inline section, distinct from help and from commands in an action menu. */
export function Disclosure({ label, children, initiallyOpen = false, accessibilityLabel = label }: PropsWithChildren<{ label: string; initiallyOpen?: boolean; accessibilityLabel?: string }>) {
  const [open, setOpen] = useState(initiallyOpen);
  const palette = useAppPalette();
  return <View style={{ gap: 8 }}>
    <Pressable accessibilityRole="button" accessibilityLabel={accessibilityLabel} accessibilityState={{ expanded: open }} onPress={() => setOpen((value) => !value)}
      style={({ pressed }) => ({ minHeight: 44, flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: 12, paddingVertical: 8, opacity: pressed ? 0.7 : 1 })}>
      <Text style={{ flex: 1, fontWeight: "600" }}>{label}</Text>
      <Svg width={18} height={18} viewBox="0 0 24 24" accessibilityElementsHidden importantForAccessibility="no">
        <Path d={open ? "M6 15l6-6 6 6" : "M6 9l6 6 6-6"} fill="none" stroke={palette.textMuted} strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" />
      </Svg>
    </Pressable>
    {open && children}
  </View>;
}
