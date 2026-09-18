import { useCallback, useContext, useRef, useState } from "react";
import { StyleSheet, Text, TextInput, View, type TextInputProps } from "react-native";

import { radius, space, useAppPalette } from "./theme";
import { FieldKeyboardContext } from "./android-keyboard-screen";

type TextFieldProps = TextInputProps & { readonly label: string };

export function TextField({
  accessibilityLabel,
  editable = true,
  label,
  multiline = false,
  onBlur,
  onFocus,
  onSelectionChange,
  onContentSizeChange,
  placeholderTextColor,
  style,
  ...props
}: TextFieldProps) {
  const palette = useAppPalette();
  const [focused, setFocused] = useState(false);
  const keyboard = useContext(FieldKeyboardContext);
  const input = useRef<TextInput | null>(null);
  const blur = keyboard?.blur;
  const inputRef = useCallback(
    (next: TextInput | null) => {
      if (next === null) blur?.(input.current);
      input.current = next;
    },
    [blur],
  );

  return (
    <View style={styles.field}>
      <Text style={[styles.label, { color: palette.text }]}>{label}</Text>
      <TextInput
        {...props}
        ref={inputRef}
        accessibilityLabel={accessibilityLabel ?? label}
        editable={editable}
        multiline={multiline}
        onBlur={(event) => {
          setFocused(false);
          keyboard?.blur(input.current);
          onBlur?.(event);
        }}
        onFocus={(event) => {
          setFocused(true);
          keyboard?.focus(input.current);
          onFocus?.(event);
        }}
        onSelectionChange={(event) => {
          keyboard?.reveal(input.current);
          onSelectionChange?.(event);
        }}
        onContentSizeChange={(event) => {
          keyboard?.reveal(input.current);
          onContentSizeChange?.(event);
        }}
        placeholderTextColor={placeholderTextColor ?? palette.textMuted}
        style={[
          styles.input,
          multiline ? styles.multiline : null,
          { backgroundColor: palette.background, color: palette.text },
          style,
          multiline && keyboard?.maximumInputHeight !== undefined
            ? {
                minHeight: Math.min(128, keyboard.maximumInputHeight),
                maxHeight: keyboard.maximumInputHeight,
              }
            : null,
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
