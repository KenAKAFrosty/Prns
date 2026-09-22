import { Pressable, StyleSheet, Text, View } from "react-native";

import { radius, space, useAppPalette } from "@/ui/theme";

/** Wrapping tabs remain usable at larger text sizes and on narrow phones. */
export function ManagementTabs({
  label,
  options,
  value,
  onChange,
}: {
  readonly label: string;
  readonly options: readonly { readonly value: string; readonly label: string }[];
  readonly value: string;
  readonly onChange: (value: string) => void;
}) {
  const palette = useAppPalette();
  return (
    <View accessibilityRole="tablist" accessibilityLabel={label} style={styles.tabs}>
      {options.map((option) => (
        <Pressable
          key={option.value}
          accessibilityRole="tab"
          accessibilityLabel={option.label}
          accessibilityState={{ selected: option.value === value }}
          onPress={() => onChange(option.value)}
          style={[
            styles.tab,
            {
              backgroundColor: option.value === value ? palette.selected : palette.surface,
              borderColor: option.value === value ? palette.focus : palette.border,
            },
          ]}
        >
          <Text style={[styles.label, { color: palette.text }]}>{option.label}</Text>
        </Pressable>
      ))}
    </View>
  );
}

const styles = StyleSheet.create({
  tabs: { flexDirection: "row", flexWrap: "wrap", gap: space.sm },
  tab: {
    borderWidth: 2,
    borderRadius: radius.sm,
    minHeight: 48,
    justifyContent: "center",
    paddingHorizontal: space.md,
    paddingVertical: space.sm,
  },
  label: { fontSize: 16, fontWeight: "600", lineHeight: 24 },
});
