import { act, render } from "@testing-library/react-native";
import { useContext } from "react";
import {
  Keyboard,
  Platform,
  ScrollView,
  View,
  type TextInput,
  type KeyboardEvent,
} from "react-native";

import {
  AndroidKeyboardScreen,
  FieldKeyboardContext,
  fieldScrollDelta,
  keyboardViewport,
} from "./android-keyboard-screen";
import { Screen } from "./primitives";
import { TextField } from "./text-field";

afterEach(() => {
  jest.restoreAllMocks();
  jest.useRealTimers();
});

test("accounts for the shell offset without double-insetting an already resized window", () => {
  expect(keyboardViewport(82, 650, 466)).toEqual({ inset: 266, height: 384 });
  expect(keyboardViewport(82, 384, 466)).toEqual({ inset: 0, height: 384 });
  expect(keyboardViewport(82, 650, null)).toEqual({ inset: 0, height: 650 });
});

test("reveals lower fields, preserves visible fields, and brings upper fields back into view", () => {
  expect(fieldScrollDelta(570, 128, 82, 384)).toBe(244);
  expect(fieldScrollDelta(120, 128, 82, 384)).toBe(0);
  expect(fieldScrollDelta(50, 48, 82, 384)).toBe(-44);
});

test("keeps the existing iOS keyboard insets and interactive dismissal", () => {
  const original = Platform.OS;
  Platform.OS = "ios";
  const view = render(
    <Screen>
      <TextField label="Message" />
    </Screen>,
  );
  const scroll = view.UNSAFE_getByType(ScrollView);
  expect(scroll.props.automaticallyAdjustKeyboardInsets).toBe(true);
  expect(scroll.props.keyboardDismissMode).toBe("interactive");
  expect(view.UNSAFE_queryByType(AndroidKeyboardScreen)).toBeNull();
  view.unmount();
  Platform.OS = original;
});

test("re-measures the focused field on keyboard and caret changes and releases listeners", async () => {
  jest.useFakeTimers();
  const listeners = new Map<string, (event: KeyboardEvent) => void>();
  const remove = jest.fn();
  const addListener = jest.spyOn(Keyboard, "addListener").mockImplementation((event, listener) => {
    listeners.set(event, listener);
    return { remove } as unknown as ReturnType<typeof Keyboard.addListener>;
  });
  const measureFrame = jest.fn(
    (callback: (x: number, y: number, width: number, height: number) => void) =>
      callback(0, 82, 412, 650),
  );
  const measureInput = jest.fn(
    (callback: (x: number, y: number, width: number, height: number) => void) =>
      callback(40, 570, 320, 128),
  );
  const scrollTo = jest.fn();
  const captured = { current: null as React.ContextType<typeof FieldKeyboardContext> };
  function Probe() {
    captured.current = useContext(FieldKeyboardContext);
    return null;
  }
  const view = render(
    <AndroidKeyboardScreen>
      <Probe />
      <TextField label="Message" multiline />
    </AndroidKeyboardScreen>,
  );
  jest
    .spyOn(view.UNSAFE_getAllByType(View)[0]?.instance as View, "measureInWindow")
    .mockImplementation(measureFrame);
  jest.spyOn(view.UNSAFE_getByType(ScrollView).instance, "scrollTo").mockImplementation(scrollTo);
  const input = { measureInWindow: measureInput } as unknown as TextInput;
  act(() => captured.current?.focus(input));
  const shown = {
    endCoordinates: { screenY: 466, height: 380, screenX: 0, width: 412 },
    duration: 0,
    easing: "keyboard",
  } as KeyboardEvent;
  await act(async () => {
    listeners.get("keyboardDidShow")?.(shown);
    jest.runOnlyPendingTimers();
  });
  expect(measureFrame).toHaveBeenCalled();
  expect(measureInput).toHaveBeenCalled();
  expect(scrollTo).toHaveBeenCalledWith({ y: 244, animated: false });
  expect(view.UNSAFE_getByType(ScrollView).props.style.marginBottom).toBe(266);
  expect(captured.current?.maximumInputHeight).toBe(360);
  const calls = measureInput.mock.calls.length;
  await act(async () => {
    captured.current?.reveal(input);
    jest.runOnlyPendingTimers();
  });
  expect(measureInput.mock.calls.length).toBeGreaterThan(calls);
  await act(async () => {
    listeners.get("keyboardDidHide")?.(shown);
    jest.runOnlyPendingTimers();
  });
  expect(view.UNSAFE_getByType(ScrollView).props.style.marginBottom).toBe(0);
  expect(captured.current?.maximumInputHeight).toBeUndefined();
  let finishMeasurement:
    | ((x: number, y: number, width: number, height: number) => void)
    | undefined;
  measureInput.mockImplementationOnce((callback) => {
    finishMeasurement = callback;
  });
  await act(async () => {
    listeners.get("keyboardDidShow")?.(shown);
    jest.runOnlyPendingTimers();
  });
  expect(finishMeasurement).toBeDefined();
  const scrollsBeforeUnmount = scrollTo.mock.calls.length;
  view.unmount();
  act(() => finishMeasurement?.(40, 570, 320, 128));
  expect(scrollTo).toHaveBeenCalledTimes(scrollsBeforeUnmount);
  expect(remove).toHaveBeenCalledTimes(2);
  addListener.mockRestore();
  jest.useRealTimers();
});
