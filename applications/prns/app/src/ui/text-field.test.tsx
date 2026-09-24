import { fireEvent, render } from "@testing-library/react-native";
import { useState } from "react";
import { StyleSheet } from "react-native";

import { TextField } from "./text-field";
import { FieldKeyboardContext } from "./android-keyboard-screen";

describe("TextField", () => {
  it("keeps its visible label while editing and preserves input callbacks", () => {
    const onFocus = jest.fn();
    const onBlur = jest.fn();
    const onSelectionChange = jest.fn();
    function Form() {
      const [value, setValue] = useState("");
      return (
        <TextField
          label="Name (optional)"
          onBlur={onBlur}
          onChangeText={setValue}
          onFocus={onFocus}
          onSelectionChange={onSelectionChange}
          placeholder="e.g. Alex"
          value={value}
        />
      );
    }
    const view = render(<Form />);
    const input = view.getByLabelText("Name (optional)");
    const originalBorder = StyleSheet.flatten(input.props.style).borderColor;

    fireEvent(input, "focus", { nativeEvent: {} });
    expect(StyleSheet.flatten(input.props.style).borderColor).not.toBe(originalBorder);
    fireEvent.changeText(input, "Alex");
    expect(view.getByDisplayValue("Alex")).toBe(input);
    expect(view.getByText("Name (optional)")).toBeTruthy();
    const selection = { nativeEvent: { selection: { start: 1, end: 3 } } };
    fireEvent(input, "selectionChange", selection);
    expect(onSelectionChange).toHaveBeenCalledWith(selection);
    fireEvent(input, "blur", { nativeEvent: {} });
    expect(StyleSheet.flatten(input.props.style).borderColor).toBe(originalBorder);
    expect(onFocus).toHaveBeenCalledTimes(1);
    expect(onBlur).toHaveBeenCalledTimes(1);
  });

  it("preserves multiline text and derives the accessible name from the label", () => {
    const onChangeText = jest.fn();
    const view = render(<TextField label="Message" multiline onChangeText={onChangeText} />);
    fireEvent.changeText(view.getByLabelText("Message"), "First line\nSecond line");
    expect(onChangeText).toHaveBeenCalledWith("First line\nSecond line");
  });

  it("notifies its Android screen about focus and caret changes while preserving caller callbacks", () => {
    const focus = jest.fn();
    const blur = jest.fn();
    const reveal = jest.fn();
    const onSelectionChange = jest.fn();
    const onContentSizeChange = jest.fn();
    const view = render(
      <FieldKeyboardContext.Provider value={{ focus, blur, reveal, maximumInputHeight: 100 }}>
        <TextField
          label="Message"
          multiline
          onSelectionChange={onSelectionChange}
          onContentSizeChange={onContentSizeChange}
        />
      </FieldKeyboardContext.Provider>,
    );
    const input = view.getByLabelText("Message");
    fireEvent(input, "focus", { nativeEvent: {} });
    const selection = { nativeEvent: { selection: { start: 2, end: 2 } } };
    const content = { nativeEvent: { contentSize: { width: 320, height: 180 } } };
    fireEvent(input, "selectionChange", selection);
    fireEvent(input, "contentSizeChange", content);
    expect(focus).toHaveBeenCalledTimes(1);
    expect(reveal).toHaveBeenCalledTimes(2);
    expect(onSelectionChange).toHaveBeenCalledWith(selection);
    expect(onContentSizeChange).toHaveBeenCalledWith(content);
    expect(StyleSheet.flatten(input.props.style)).toMatchObject({ minHeight: 100, maxHeight: 100 });
    fireEvent(input, "blur", { nativeEvent: {} });
    expect(blur).toHaveBeenCalledTimes(1);
    view.unmount();
    expect(blur).toHaveBeenCalledTimes(2);
  });
});
