import { fireEvent, render } from "@testing-library/react-native";
import { StyleSheet } from "react-native";

import { ManagementTabs } from "./management-tabs";

let mockFontScale = 1;
let mockWidth = 360;
jest.mock("react-native/Libraries/Utilities/useWindowDimensions", () => ({
  __esModule: true,
  default: () => ({ width: mockWidth, height: 740, scale: 4, fontScale: mockFontScale }),
}));

const options = [
  { value: "interfaces", label: "Interfaces" },
  { value: "device", label: "Device" },
  { value: "information", label: "Information", shortLabel: "Info" },
  { value: "access", label: "Access" },
];

beforeEach(() => {
  mockFontScale = 1;
  mockWidth = 360;
});

test("four compact tabs fit normal phone width without reducing touch height or accessibility labels", () => {
  const onChange = jest.fn();
  const view = render(
    <ManagementTabs
      label="Settings sections"
      options={options}
      value="interfaces"
      onChange={onChange}
    />,
  );
  const tabs = view.getAllByRole("tab");
  let minimumWidth = 0;
  for (const tab of tabs) {
    const style = StyleSheet.flatten(tab.props.style);
    minimumWidth += style.minWidth;
    expect(style.minHeight).toBeGreaterThanOrEqual(48);
    expect(style.flexGrow).toBe(1);
  }
  const gap = StyleSheet.flatten(
    view.UNSAFE_getByProps({ accessibilityRole: "tablist" }).props.style,
  ).gap;
  expect(minimumWidth + gap * 3).toBeLessThanOrEqual(360 - 32);
  expect(view.getByText("Info")).toBeTruthy();
  expect(view.getByRole("tab", { name: "Information" })).toBeTruthy();
  expect(view.getByRole("tab", { name: "Interfaces" }).props.accessibilityState.selected).toBe(
    true,
  );
  fireEvent.press(view.getByRole("tab", { name: "Information" }));
  expect(onChange).toHaveBeenCalledWith("information");
});

test("large type increases tab width and wraps instead of shrinking or truncating text", () => {
  mockFontScale = 2;
  const view = render(
    <ManagementTabs
      label="Settings sections"
      options={options}
      value="access"
      onChange={jest.fn()}
    />,
  );
  expect(
    StyleSheet.flatten(view.UNSAFE_getByProps({ accessibilityRole: "tablist" }).props.style)
      .flexWrap,
  ).toBe("wrap");
  expect(
    StyleSheet.flatten(view.getByRole("tab", { name: "Interfaces" }).props.style).minWidth,
  ).toBe(128);
  expect(view.getByText("Interfaces").props.numberOfLines).toBeUndefined();
  expect(view.getByText("Interfaces").props.allowFontScaling).not.toBe(false);
});

test("narrow phones arrange tabs in pairs so names have room", () => {
  mockWidth = 320;
  const view = render(
    <ManagementTabs
      label="Settings sections"
      options={options}
      value="interfaces"
      onChange={jest.fn()}
    />,
  );
  expect(
    StyleSheet.flatten(view.getByRole("tab", { name: "Interfaces" }).props.style).flexBasis,
  ).toBe("45%");
});
