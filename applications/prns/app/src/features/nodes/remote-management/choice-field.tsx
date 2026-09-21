import { Pressable, StyleSheet, Text, View } from "react-native";

import { BodyText } from "@/ui/primitives";
import { radius, space, useAppPalette } from "@/ui/theme";

export function ChoiceField<T extends number>({
  label,
  options,
  value,
  disabled,
  onChange,
}: {
  readonly label: string;
  readonly options: readonly { readonly value: T; readonly label: string }[];
  readonly value: T | undefined;
  readonly disabled: boolean;
  readonly onChange: (value: T) => void;
}) {
  const palette = useAppPalette();
  return (
    <View style={styles.field}>
      <BodyText>{label}</BodyText>
      <View accessibilityRole="radiogroup" accessibilityLabel={label} style={styles.choices}>
        {options.map((option) => (
          <Pressable
            key={option.value}
            accessibilityRole="radio"
            accessibilityLabel={option.label}
            accessibilityState={{ checked: option.value === value, disabled }}
            disabled={disabled}
            onPress={() => onChange(option.value)}
            style={[
              styles.choice,
              {
                backgroundColor: option.value === value ? palette.selected : palette.surface,
                borderColor: option.value === value ? palette.focus : palette.border,
                opacity: disabled ? 0.5 : 1,
              },
            ]}
          >
            <Text style={[styles.label, { color: palette.text }]}>{option.label}</Text>
          </Pressable>
        ))}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  field: { gap: space.sm },
  choices: { flexDirection: "row", flexWrap: "wrap", gap: space.sm },
  choice: {
    borderWidth: 2,
    borderRadius: radius.sm,
    minHeight: 48,
    justifyContent: "center",
    paddingHorizontal: space.md,
    paddingVertical: space.sm,
  },
  label: { fontSize: 16, lineHeight: 24 },
});
