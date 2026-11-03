import { useContext } from "react";
import { act, fireEvent, render } from "@testing-library/react-native";
import type { KeyboardEvent, TextInputInstance } from "react-native";
import { IOSKeyboardScreen } from "../src/ui/ios-keyboard-screen";
import { FieldKeyboardContext, type FieldKeyboard } from "../src/ui/field-keyboard";

type Measure = (x: number, y: number, width: number, height: number) => void;
const mockListeners = new Map<string, (event: KeyboardEvent) => void>();
const mockFrameMeasure = jest.fn((callback: Measure) => callback(0, 180, 393, 550));
const mockScroll = jest.fn();
let mockKeyboard: FieldKeyboard;
let mockMetrics: { screenY: number } | undefined;
jest.mock("react-native", () => {
  const React = require("react");
  const native = jest.requireActual("react-native");
  const overrides: Record<string, unknown> = {
    View: React.forwardRef((props: object, ref: unknown) => {
      React.useImperativeHandle(ref, () => ({ measureInWindow: mockFrameMeasure }));
      return React.createElement(native.View, props);
    }),
    ScrollView: React.forwardRef((props: object, ref: unknown) => {
      React.useImperativeHandle(ref, () => ({ scrollTo: mockScroll }));
      return React.createElement(native.View, { ...props, testID: "keyboard-scroll" });
    }),
    Keyboard: { metrics: () => mockMetrics, addListener: (name: string, listener: (event: KeyboardEvent) => void) => {
      mockListeners.set(name, listener);
      return { remove: () => mockListeners.delete(name) };
    } },
  };
  return new Proxy(native, { get: (target, key: string) => key in overrides ? overrides[key] : target[key] });
});
function Fields() { mockKeyboard = useContext(FieldKeyboardContext)!; return null; }
function input(top: number, height: number) {
  return { measureInWindow: jest.fn((callback: Measure) => callback(16, top, 286, height)) } as unknown as TextInputInstance;
}
const frames = new Map<number, FrameRequestCallback>();
let nextFrame = 0;
beforeEach(() => {
  mockMetrics = undefined; mockListeners.clear(); frames.clear(); mockScroll.mockClear();
  mockFrameMeasure.mockImplementation((callback) => callback(0, 180, 393, 550));
  jest.spyOn(global, "requestAnimationFrame").mockImplementation((callback) => { const id = ++nextFrame; frames.set(id, callback); return id; });
  jest.spyOn(global, "cancelAnimationFrame").mockImplementation((id) => { if (id != null) frames.delete(id); });
});
afterEach(() => { jest.restoreAllMocks(); });
async function settle() {
  await act(async () => { const pending = [...frames.values()]; frames.clear(); pending.forEach((callback) => callback(0)); });
}
async function keyboard(name: string, screenY = 524) {
  await act(async () => mockListeners.get(name)?.({ endCoordinates: { screenY, height: 328 } } as KeyboardEvent));
  await settle();
}

test("after native keyboard settling, the whole padded input clears the prediction bar without a second inset", async () => {
  const view = await render(<IOSKeyboardScreen style={{ flex: 1 }}><Fields /></IOSKeyboardScreen>);
  const field = input(494, 48);
  await act(async () => { mockKeyboard.focus(field); });
  await settle();
  expect(mockScroll).not.toHaveBeenCalled();
  await fireEvent.scroll(view.getByTestId("keyboard-scroll"), { nativeEvent: { contentOffset: { y: 120 } } });
  await keyboard("keyboardDidShow");
  // The caret can fit at 524 while its padded input and adjacent Send button do not.
  expect(mockScroll).toHaveBeenLastCalledWith({ y: 150, animated: false });
  expect(view.getByTestId("keyboard-scroll").props.automaticallyAdjustKeyboardInsets).toBe(true);
  expect(mockKeyboard.maximumInputHeight).toBe(320);
  const visible = input(450, 48);
  mockScroll.mockClear();
  await act(async () => { mockKeyboard.focus(visible); });
  await settle();
  expect(mockScroll).not.toHaveBeenCalled();
  await view.unmount();
});

test("delayed measurements cannot scroll a replaced or blurred input", async () => {
  mockMetrics = { screenY: 524 };
  const view = await render(<IOSKeyboardScreen><Fields /></IOSKeyboardScreen>);
  let late!: Measure;
  const old = { measureInWindow: (callback: Measure) => { late = callback; } } as TextInputInstance;
  await act(async () => { mockKeyboard.focus(old); });
  await settle();
  await act(async () => { mockKeyboard.focus(input(460, 48)); });
  await settle();
  await act(async () => late(16, 650, 286, 48));
  expect(mockScroll).not.toHaveBeenCalled();
  await act(async () => { mockKeyboard.focus(old); });
  await settle();
  await act(async () => { mockKeyboard.blur(old); late(16, 650, 286, 48); });
  expect(mockScroll).not.toHaveBeenCalled();
  await view.unmount();
});

test("keyboard frame changes and input growth reveal the current field; hide and unmount retire measurements", async () => {
  mockMetrics = { screenY: 524 };
  const view = await render(<IOSKeyboardScreen><Fields /></IOSKeyboardScreen>);
  const field = input(450, 48);
  await act(async () => { mockKeyboard.focus(field); });
  await settle();
  expect(mockScroll).not.toHaveBeenCalled();
  await keyboard("keyboardDidChangeFrame", 480);
  expect(mockScroll).toHaveBeenLastCalledWith({ y: 30, animated: false });
  mockScroll.mockClear();
  let late!: Measure;
  jest.mocked(field.measureInWindow).mockImplementation((callback) => { late = callback; });
  await act(async () => { mockKeyboard.reveal(field); });
  await settle();
  await keyboard("keyboardDidHide");
  await act(async () => late(16, 450, 286, 140));
  expect(mockScroll).not.toHaveBeenCalled();
  expect(mockKeyboard.maximumInputHeight).toBeUndefined();
  await keyboard("keyboardDidShow");
  await view.unmount();
  await act(async () => late(16, 650, 286, 48));
  expect(mockScroll).not.toHaveBeenCalled();
  expect(mockListeners.size).toBe(0);
  expect(frames.size).toBe(0);
});
