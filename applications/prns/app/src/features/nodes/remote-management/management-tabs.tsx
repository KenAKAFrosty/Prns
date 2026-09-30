import { Pressable, StyleSheet, Text, View, useWindowDimensions } from "react-native";

import { radius, space, useAppPalette } from "@/ui/theme";

/** Wrapping tabs remain usable at larger text sizes and on narrow phones. */
export function ManagementTabs({
  label,
  options,
  value,
  onChange,
}: {
  readonly label: string;
  readonly options: readonly {
    readonly value: string;
    readonly label: string;
    readonly shortLabel?: string;
  }[];
  readonly value: string;
  readonly onChange: (value: string) => void;
}) {
  const palette = useAppPalette();
  const { width, fontScale } = useWindowDimensions();
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
              minWidth: 64 * fontScale,
              flexBasis: width / fontScale < 360 ? "45%" : 0,
            },
          ]}
        >
          <Text style={[styles.label, { color: palette.text }]}>
            {option.shortLabel ?? option.label}
          </Text>
        </Pressable>
      ))}
    </View>
  );
}

const styles = StyleSheet.create({
  tabs: { flexDirection: "row", flexWrap: "wrap", gap: space.xs },
  tab: {
    borderWidth: 1,
    borderRadius: radius.sm,
    flexBasis: 0,
    flexGrow: 1,
    minHeight: 48,
    justifyContent: "center",
    paddingHorizontal: 6,
    paddingVertical: space.sm,
  },
  label: { fontSize: 14, fontWeight: "600", lineHeight: 20, textAlign: "center" },
});
