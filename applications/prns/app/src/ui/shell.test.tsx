import { render } from "@testing-library/react-native";
import type { ReactNode } from "react";
import { StyleSheet } from "react-native";

import { ShellLayout } from "./shell";

let mockPathname = "/inbox";
let mockWidth = 390;
let mockFontScale = 1;

jest.mock("expo-router", () => ({
  Link: ({ children }: { readonly children: ReactNode }) => children,
  Slot: () => null,
  usePathname: () => mockPathname,
}));

jest.mock("react-native/Libraries/Utilities/useWindowDimensions", () => ({
  __esModule: true,
  default: () => ({ fontScale: mockFontScale, height: 844, scale: 3, width: mockWidth }),
}));

jest.mock("@/state/scaffold-state-context", () => ({
  useScaffoldState: () => ({
    state: { scaffoldSchema: 2, showUnavailableFeatures: true },
  }),
}));

describe("shell accessibility", () => {
  beforeEach(() => {
    mockPathname = "/inbox";
    mockWidth = 390;
    mockFontScale = 1;
  });

  it("exposes compact shell landmarks and the current page", () => {
    const view = render(<ShellLayout />);

    expect(view.UNSAFE_getByProps({ role: "banner" })).toBeTruthy();
    expect(
      view.UNSAFE_getByProps({ accessibilityLabel: "Primary navigation", role: "navigation" }),
    ).toBeTruthy();
    expect(view.UNSAFE_getByProps({ role: "main" })).toBeTruthy();
    expect(view.UNSAFE_queryAllByProps({ role: "complementary" })).toHaveLength(0);

    expect(view.getByRole("link", { name: "Inbox, current page" })).toBeTruthy();
    expect(view.getByRole("link", { name: "Contacts" })).toBeTruthy();
  });

  it("uses the wide rail without adding a generic context panel", () => {
    mockPathname = "/explore/nomadnet/page";
    mockWidth = 1200;

    const view = render(<ShellLayout />);

    expect(
      view.UNSAFE_getByProps({ accessibilityLabel: "Primary navigation", role: "navigation" }),
    ).toBeTruthy();
    expect(view.UNSAFE_getByProps({ role: "main" })).toBeTruthy();
    expect(view.UNSAFE_queryAllByProps({ accessibilityLabel: "Workspace context" })).toHaveLength(
      0,
    );
    expect(view.getByRole("link", { name: "NomadNet, current page" })).toBeTruthy();
  });

  it("gives enlarged navigation labels more width instead of shrinking their text", () => {
    mockWidth = 320;
    mockFontScale = 1.5;
    const view = render(<ShellLayout />);
    const contact = view.getByRole("link", { name: "Contacts" });
    expect(StyleSheet.flatten(contact.props.style)).toMatchObject({
      flexBasis: "30%",
      minHeight: 48,
    });
    expect(view.getByText("Contacts").props.allowFontScaling).not.toBe(false);
    expect(view.getByText("Contacts").props.numberOfLines).toBeUndefined();
  });
});
