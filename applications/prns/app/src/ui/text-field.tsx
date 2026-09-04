import { useState } from "react";
import { StyleSheet, Text, TextInput, View, type TextInputProps } from "react-native";

import { radius, space, useAppPalette } from "./theme";

type TextFieldProps = TextInputProps & { readonly label: string };

export function TextField({
  accessibilityLabel,
  editable = true,
  label,
  multiline = false,
  onBlur,
  onFocus,
  placeholderTextColor,
  style,
  ...props
}: TextFieldProps) {
  const palette = useAppPalette();
  const [focused, setFocused] = useState(false);

  return (
    <View style={styles.field}>
      <Text style={[styles.label, { color: palette.text }]}>{label}</Text>
      <TextInput
        {...props}
        accessibilityLabel={accessibilityLabel ?? label}
        editable={editable}
        multiline={multiline}
        onBlur={(event) => {
          setFocused(false);
          onBlur?.(event);
        }}
        onFocus={(event) => {
          setFocused(true);
          onFocus?.(event);
        }}
        placeholderTextColor={placeholderTextColor ?? palette.textMuted}
        style={[
          styles.input,
          multiline ? styles.multiline : null,
          { backgroundColor: palette.background, color: palette.text },
          style,
          {
            borderColor: focused && editable ? palette.focus : palette.border,
            opacity: editable ? 1 : 0.6,
          },
        ]}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  field: { gap: space.sm, minWidth: 0, paddingVertical: space.xs },
  label: { fontSize: 14, fontWeight: "600", lineHeight: 20 },
  input: {
    borderRadius: radius.sm,
    borderWidth: 1,
    fontSize: 16,
    minHeight: 48,
    minWidth: 0,
    paddingHorizontal: space.md,
    paddingVertical: 12,
  },
  multiline: { minHeight: 128, textAlignVertical: "top" },
});
