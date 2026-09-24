import { fireEvent, render } from "@testing-library/react-native";
import { StyleSheet, View } from "react-native";

import { ActionRow, Badge, Button, CardHeader, CardSection, KeyValue } from "./primitives";

let mockWidth = 390;
let mockFontScale = 1;

jest.mock("react-native/Libraries/Utilities/useWindowDimensions", () => ({
  __esModule: true,
  default: () => ({ width: mockWidth, height: 844, scale: 3, fontScale: mockFontScale }),
}));

describe("compact grouped primitives", () => {
  beforeEach(() => {
    mockWidth = 390;
    mockFontScale = 1;
  });

  it("puts short values beside labels without truncation or losing selection", () => {
    const view = render(<KeyValue label="Frequency" value="915 MHz" />);
    const value = view.getByText("915 MHz");
    expect(StyleSheet.flatten(view.UNSAFE_getByType(View).props.style).flexDirection).toBe("row");
    expect(StyleSheet.flatten(value.props.style).textAlign).toBe("right");
    expect(value.props.selectable).toBe(true);
    expect(value.props.numberOfLines).toBeUndefined();
    expect(value.props.allowFontScaling).not.toBe(false);
  });

  it.each([
    [320, 1, "22 dBm"],
    [390, 1.5, "22 dBm"],
    [390, 1, "568151a94a182261c37f14c2f397f2b1"],
    [390, 1, "First line\nSecond line"],
  ])("stacks values for width %s, font scale %s and long content", (width, fontScale, value) => {
    mockWidth = width;
    mockFontScale = fontScale;
    const view = render(<KeyValue label="Value" value={value} />);
    expect(StyleSheet.flatten(view.UNSAFE_getByType(View).props.style).flexDirection).toBe(
      "column",
    );
    expect(StyleSheet.flatten(view.getByText(value).props.style).textAlign).toBe("left");
  });

  it("keeps header semantics and separates sections with a visible border", () => {
    const view = render(
      <>
        <CardHeader title="Bluetooth">
          <Badge>Connected</Badge>
        </CardHeader>
        <CardSection title="Connection details">
          <KeyValue label="Power" value="On" />
        </CardSection>
      </>,
    );
    expect(view.getByRole("header", { name: "Bluetooth" })).toBeTruthy();
    expect(view.getByRole("header", { name: "Connection details" })).toBeTruthy();
    expect(view.getByText("Connected")).toBeTruthy();
    expect(
      view
        .UNSAFE_getAllByType(View)
        .some(
          (node) =>
            StyleSheet.flatten(node.props.style)?.borderTopWidth === StyleSheet.hairlineWidth,
        ),
    ).toBe(true);
  });

  it("groups controls without reducing touch targets or reserving empty slots", () => {
    const onPress = jest.fn();
    const view = render(
      <ActionRow>
        <Button onPress={onPress}>Refresh</Button>
        {false && <Button>Hidden</Button>}
        <Button disabled>Save</Button>
      </ActionRow>,
    );
    const buttons = view.getAllByRole("button");
    expect(buttons).toHaveLength(2);
    expect(
      view
        .UNSAFE_getAllByType(View)
        .filter((node) => StyleSheet.flatten(node.props.style)?.flexBasis === 144),
    ).toHaveLength(2);
    expect(StyleSheet.flatten(buttons[0]?.props.style).minHeight).toBeGreaterThanOrEqual(48);
    fireEvent.press(view.getByRole("button", { name: "Refresh" }));
    expect(onPress).toHaveBeenCalledTimes(1);
    expect(view.getByRole("button", { name: "Save" }).props.accessibilityState.disabled).toBe(true);
  });

  it("stacks related controls for large text", () => {
    mockFontScale = 1.6;
    const view = render(
      <ActionRow>
        <Button>Refresh</Button>
        <Button>Save</Button>
      </ActionRow>,
    );
    expect(
      view
        .UNSAFE_getAllByType(View)
        .filter((node) => StyleSheet.flatten(node.props.style)?.flexBasis === "100%"),
    ).toHaveLength(2);
  });
});
