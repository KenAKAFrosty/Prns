import { fireEvent, render } from "@testing-library/react-native";
import { useState } from "react";
import { StyleSheet } from "react-native";

import { TextField } from "./text-field";

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
});
