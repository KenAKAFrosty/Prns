import { createContext } from "react";
import type { TextInputInstance } from "react-native";

export const FIELD_KEYBOARD_GAP = 12;

export type FieldKeyboard = {
  readonly focus: (input: TextInputInstance | null) => void;
  readonly blur: (input: TextInputInstance | null) => void;
  readonly reveal: (input: TextInputInstance | null) => void;
  readonly maximumInputHeight: number | undefined;
};

export const FieldKeyboardContext = createContext<FieldKeyboard | null>(null);

/** Screen coordinates, not keyboard height: handles both resized and edge-to-edge windows. */
export function keyboardViewport(top: number, height: number, keyboardTop: number | null) {
  const bottom = top + height;
  const visibleBottom =
    keyboardTop === null ? bottom : Math.max(top, Math.min(bottom, keyboardTop));
  return { inset: bottom - visibleBottom, height: visibleBottom - top };
}

export function fieldScrollDelta(
  top: number,
  height: number,
  viewportTop: number,
  viewportHeight: number,
) {
  const upper = viewportTop + FIELD_KEYBOARD_GAP;
  const lower = viewportTop + viewportHeight - FIELD_KEYBOARD_GAP;
  if (top < upper) return top - upper;
  return Math.max(0, top + height - lower);
}
