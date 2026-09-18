import {
  createContext,
  type PropsWithChildren,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Keyboard, ScrollView, View, type ScrollViewProps, type TextInput } from "react-native";

const GAP = 12;

type FieldKeyboard = {
  readonly focus: (input: TextInput | null) => void;
  readonly blur: (input: TextInput | null) => void;
  readonly reveal: (input: TextInput | null) => void;
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
  const upper = viewportTop + GAP;
  const lower = viewportTop + viewportHeight - GAP;
  if (top < upper) return top - upper;
  return Math.max(0, top + height - lower);
}

export function AndroidKeyboardScreen({
  children,
  style,
  contentContainerStyle,
}: PropsWithChildren<Pick<ScrollViewProps, "style" | "contentContainerStyle">>) {
  const frame = useRef<View>(null);
  const scroll = useRef<ScrollView>(null);
  const input = useRef<TextInput | null>(null);
  const offset = useRef(0);
  const keyboardTop = useRef<number | null>(Keyboard.metrics()?.screenY ?? null);
  const alive = useRef(true);
  const pendingFrame = useRef<number | null>(null);
  const measurement = useRef(0);
  const [viewport, setViewport] = useState({ inset: 0, height: 0, keyboardVisible: false });

  const measure = useCallback(() => {
    const request = ++measurement.current;
    frame.current?.measureInWindow((_x, top, _width, height) => {
      if (!alive.current || measurement.current !== request) return;
      const next = {
        ...keyboardViewport(top, height, keyboardTop.current),
        keyboardVisible: keyboardTop.current !== null,
      };
      setViewport((previous) =>
        previous.inset === next.inset &&
        previous.height === next.height &&
        previous.keyboardVisible === next.keyboardVisible
          ? previous
          : next,
      );
      const focused = input.current;
      if (focused === null || keyboardTop.current === null || next.height <= GAP * 2) return;
      focused.measureInWindow((_inputX, inputTop, _inputWidth, inputHeight) => {
        if (!alive.current || measurement.current !== request || input.current !== focused) return;
        const delta = fieldScrollDelta(inputTop, inputHeight, top, next.height);
        if (Math.abs(delta) > 1)
          scroll.current?.scrollTo({ y: Math.max(0, offset.current + delta), animated: false });
      });
    });
  }, []);

  const schedule = useCallback(() => {
    if (!alive.current) return;
    if (pendingFrame.current !== null) cancelAnimationFrame(pendingFrame.current);
    pendingFrame.current = requestAnimationFrame(() => {
      pendingFrame.current = null;
      measure();
    });
  }, [measure]);

  useEffect(() => {
    alive.current = true;
    const shown = Keyboard.addListener("keyboardDidShow", (event) => {
      keyboardTop.current = event.endCoordinates.screenY;
      schedule();
    });
    const hidden = Keyboard.addListener("keyboardDidHide", () => {
      keyboardTop.current = null;
      schedule();
    });
    return () => {
      alive.current = false;
      measurement.current += 1;
      if (pendingFrame.current !== null) cancelAnimationFrame(pendingFrame.current);
      shown.remove();
      hidden.remove();
    };
  }, [schedule]);

  const focus = useCallback(
    (focused: TextInput | null) => {
      input.current = focused;
      schedule();
    },
    [schedule],
  );
  const blur = useCallback((previous: TextInput | null) => {
    if (input.current === previous) {
      input.current = null;
      measurement.current += 1;
    }
  }, []);
  const reveal = useCallback(
    (focused: TextInput | null) => {
      if (input.current === focused) schedule();
    },
    [schedule],
  );
  const context = useMemo(
    () => ({
      focus,
      blur,
      reveal,
      maximumInputHeight:
        viewport.keyboardVisible && viewport.height > 0
          ? Math.max(48, viewport.height - GAP * 2)
          : undefined,
    }),
    [blur, focus, reveal, viewport.height, viewport.keyboardVisible],
  );

  return (
    <FieldKeyboardContext.Provider value={context}>
      <View ref={frame} style={style} onLayout={schedule} collapsable={false}>
        <ScrollView
          ref={scroll}
          style={{ flex: 1, marginBottom: viewport.inset }}
          contentContainerStyle={contentContainerStyle}
          keyboardDismissMode="on-drag"
          keyboardShouldPersistTaps="handled"
          onLayout={schedule}
          onContentSizeChange={schedule}
          onScroll={(event) => {
            offset.current = event.nativeEvent.contentOffset.y;
          }}
          scrollEventThrottle={16}
        >
          {children}
        </ScrollView>
      </View>
    </FieldKeyboardContext.Provider>
  );
}
