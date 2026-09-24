import { fireEvent, render } from "@testing-library/react-native";
import { StyleSheet } from "react-native";

import { ChoiceField } from "./choice-field";
import { interfaceModes, loraRegions } from "./interface-format";
import { LoRaEditor } from "./lora-editor";

test("long lists show the selected value without occupying the form with every option", () => {
  const onChange = jest.fn();
  const selected = loraRegions[0];
  const view = render(
    <ChoiceField
      label="Region"
      options={loraRegions}
      value={selected.value}
      disabled={false}
      onChange={onChange}
    />,
  );
  expect(view.getByText(`Region: ${selected.label}`)).toBeTruthy();
  expect(view.queryAllByRole("radio")).toHaveLength(0);
  const disclosure = view.getByRole("button", { name: "Change region" });
  expect(disclosure.props.accessibilityState.expanded).toBe(false);
  expect(StyleSheet.flatten(disclosure.props.style).minHeight).toBeGreaterThanOrEqual(48);
  fireEvent.press(disclosure);
  expect(onChange).not.toHaveBeenCalled();
  expect(view.getAllByRole("radio")).toHaveLength(loraRegions.length);
  expect(view.getByRole("radio", { name: selected.label }).props.accessibilityState.checked).toBe(
    true,
  );
  fireEvent.press(view.getByRole("button", { name: "Close choices" }));
  expect(onChange).not.toHaveBeenCalled();
  expect(view.queryAllByRole("radio")).toHaveLength(0);
});

test("an explicit choice updates the draft and collapses the options", () => {
  const onChange = jest.fn();
  const selected = loraRegions[4];
  const view = render(
    <ChoiceField
      label="Region"
      options={loraRegions}
      value={undefined}
      disabled={false}
      onChange={onChange}
    />,
  );
  expect(view.getByText("Region: Not selected")).toBeTruthy();
  fireEvent.press(view.getByRole("button", { name: "Choose region" }));
  fireEvent.press(view.getByRole("radio", { name: selected.label }));
  expect(onChange).toHaveBeenCalledTimes(1);
  expect(onChange).toHaveBeenCalledWith(selected.value);
  expect(view.queryAllByRole("radio")).toHaveLength(0);
  view.rerender(
    <ChoiceField
      label="Region"
      options={loraRegions}
      value={selected.value}
      disabled={false}
      onChange={onChange}
    />,
  );
  expect(view.getByText(`Region: ${selected.label}`)).toBeTruthy();
});

test("busy fields cannot expand or change an already expanded selection", () => {
  const onChange = jest.fn();
  const props = { label: "Region", options: loraRegions, value: loraRegions[0].value, onChange };
  const view = render(<ChoiceField {...props} disabled />);
  fireEvent.press(view.getByRole("button", { name: "Change region" }));
  expect(view.queryAllByRole("radio")).toHaveLength(0);
  view.rerender(<ChoiceField {...props} disabled={false} />);
  fireEvent.press(view.getByRole("button", { name: "Change region" }));
  view.rerender(<ChoiceField {...props} disabled />);
  for (const radio of view.getAllByRole("radio")) {
    expect(radio).toBeDisabled();
    fireEvent.press(radio);
  }
  expect(onChange).not.toHaveBeenCalled();
});

test("shorter interface mode lists keep their existing radio presentation", () => {
  const view = render(
    <ChoiceField
      label="Interface mode"
      options={interfaceModes}
      value={interfaceModes[0].value}
      disabled={false}
      onChange={jest.fn()}
    />,
  );
  expect(view.getAllByRole("radio")).toHaveLength(interfaceModes.length);
  expect(view.queryByRole("button", { name: "Change interface mode" })).toBeNull();
});

test("choosing a radio region does not save or send a node change", () => {
  const onSave = jest.fn();
  const view = render(<LoRaEditor busy={false} onSave={onSave} />);
  fireEvent.press(view.getByRole("button", { name: "Choose region" }));
  fireEvent.press(view.getByRole("radio", { name: "US 915" }));
  expect(view.getByText("Region: US 915")).toBeTruthy();
  expect(view.queryAllByRole("radio")).toHaveLength(0);
  expect(onSave).not.toHaveBeenCalled();
  expect(view.getByRole("button", { name: "Save LoRa settings" })).toBeDisabled();
});
