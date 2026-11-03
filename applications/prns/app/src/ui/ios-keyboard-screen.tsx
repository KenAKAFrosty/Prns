import { useCallback, useEffect, useMemo, useRef, useState, type PropsWithChildren } from "react";
import { Keyboard, ScrollView, View, type ScrollViewProps, type TextInputInstance, type ViewInstance, type ScrollViewInstance } from "react-native";
import { FieldKeyboardContext, FIELD_KEYBOARD_GAP, fieldScrollDelta, keyboardViewport } from "./field-keyboard";

/** Native keyboard insets reveal the caret; this also reveals the padded input frame. */
export function IOSKeyboardScreen({ children, style, contentContainerStyle }:
  PropsWithChildren<Pick<ScrollViewProps, "style" | "contentContainerStyle">>) {
  const frame = useRef<ViewInstance>(null);
  const scroll = useRef<ScrollViewInstance>(null);
  const input = useRef<TextInputInstance | null>(null);
  const offset = useRef(0);
  const keyboardTop = useRef<number | null>(Keyboard.metrics()?.screenY ?? null);
  const alive = useRef(false);
  const pendingFrame = useRef<number | null>(null);
  const measurement = useRef(0);
  const [maximumInputHeight, setMaximumInputHeight] = useState<number>();

  const reveal = useCallback(() => {
    const ticket = ++measurement.current;
    const focused = input.current;
    const keyboard = keyboardTop.current;
    if (!alive.current || focused === null || keyboard === null) return;
    const current = () => alive.current && ticket === measurement.current && input.current === focused;
    frame.current?.measureInWindow((_x, top, _width, height) => {
      if (!current()) return;
      const viewport = keyboardViewport(top, height, keyboard);
      if (viewport.height <= FIELD_KEYBOARD_GAP * 2) return;
      setMaximumInputHeight(Math.max(48, viewport.height - FIELD_KEYBOARD_GAP * 2));
      focused.measureInWindow((_inputX, inputTop, _inputWidth, inputHeight) => {
        if (!current()) return;
        const delta = fieldScrollDelta(inputTop, inputHeight, top, viewport.height);
        if (Math.abs(delta) > 1)
          scroll.current?.scrollTo({ y: Math.max(0, offset.current + delta), animated: false });
      });
    });
  }, []);
  const schedule = useCallback(() => {
    measurement.current++;
    if (!alive.current) return;
    if (pendingFrame.current !== null) cancelAnimationFrame(pendingFrame.current);
    pendingFrame.current = requestAnimationFrame(() => {
      pendingFrame.current = null;
      reveal();
    });
  }, [reveal]);
  useEffect(() => {
    alive.current = true;
    const shown = Keyboard.addListener("keyboardDidShow", (event) => {
      keyboardTop.current = event.endCoordinates.screenY;
      schedule();
    });
    const changed = Keyboard.addListener("keyboardDidChangeFrame", (event) => {
      keyboardTop.current = event.endCoordinates.height > 0 ? event.endCoordinates.screenY : null;
      schedule();
    });
    const hidden = Keyboard.addListener("keyboardDidHide", () => {
      keyboardTop.current = null;
      measurement.current++;
      setMaximumInputHeight(undefined);
    });
    schedule();
    return () => {
      alive.current = false;
      measurement.current++;
      if (pendingFrame.current !== null) cancelAnimationFrame(pendingFrame.current);
      shown.remove(); changed.remove(); hidden.remove();
    };
  }, [schedule]);
  const context = useMemo(() => ({
    focus: (focused: TextInputInstance | null) => { input.current = focused; schedule(); },
    blur: (focused: TextInputInstance | null) => {
      if (input.current === focused) { input.current = null; measurement.current++; }
    },
    reveal: (focused: TextInputInstance | null) => { if (input.current === focused) schedule(); },
    maximumInputHeight,
  }), [maximumInputHeight, schedule]);

  return <FieldKeyboardContext.Provider value={context}>
    <View ref={frame} style={style} onLayout={schedule} collapsable={false}>
      <ScrollView ref={scroll} style={{ flex: 1 }} contentContainerStyle={contentContainerStyle}
        automaticallyAdjustKeyboardInsets keyboardDismissMode="interactive" keyboardShouldPersistTaps="handled"
        onContentSizeChange={schedule} onScroll={(event) => { offset.current = event.nativeEvent.contentOffset.y; }} scrollEventThrottle={16}>
        {children}
      </ScrollView>
    </View>
  </FieldKeyboardContext.Provider>;
}
