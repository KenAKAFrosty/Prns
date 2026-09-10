import { render } from "@testing-library/react-native";
import { useEffect, type ReactNode } from "react";
import { Text } from "react-native";

import { MasterDetailLayout } from "./master-detail-layout";

let mockPathname = "/inbox";
let mockWidth = 390;
let mockSlotMounts = 0;

function MockSlotProbe() {
  useEffect(() => {
    mockSlotMounts += 1;
  }, []);
  return <Text>Selected route</Text>;
}

jest.mock("expo-router", () => ({
  Slot: () => <MockSlotProbe />,
  usePathname: () => mockPathname,
}));

jest.mock("react-native/Libraries/Utilities/useWindowDimensions", () => ({
  __esModule: true,
  default: () => ({ fontScale: 1, height: 844, scale: 3, width: mockWidth }),
}));

function layout(master: ReactNode = <Text>Inbox list</Text>) {
  return (
    <MasterDetailLayout
      emptyDescription="Choose a conversation from the list or start a new message."
      emptyTitle="Select a conversation"
      master={master}
      rootPath="/inbox"
      sectionLabel="Inbox"
    />
  );
}

describe("MasterDetailLayout", () => {
  beforeEach(() => {
    mockPathname = "/inbox";
    mockSlotMounts = 0;
    mockWidth = 390;
  });

  it("keeps compact and medium routes on one surface", () => {
    mockPathname = "/inbox/conversation/0011";

    const view = render(layout());

    expect(view.getByText("Selected route")).toBeTruthy();
    expect(view.queryByText("Inbox list")).toBeNull();
    expect(view.UNSAFE_getByProps({ accessibilityLabel: "Inbox content" })).toBeTruthy();

    mockWidth = 1199;
    view.rerender(layout());

    expect(view.queryByText("Inbox list")).toBeNull();
    expect(view.UNSAFE_getByProps({ accessibilityLabel: "Inbox content" })).toBeTruthy();
  });

  it("shows the root list with a useful empty detail pane on wide layouts", () => {
    mockWidth = 1200;

    const view = render(layout());

    expect(
      view.UNSAFE_getByProps({ accessibilityLabel: "Inbox list", role: "region" }),
    ).toBeTruthy();
    expect(
      view.UNSAFE_getByProps({ accessibilityLabel: "Inbox detail", role: "region" }),
    ).toBeTruthy();
    expect(view.getByText("Selected route")).toBeTruthy();
    expect(view.getByText("Select a conversation")).toBeTruthy();
  });

  it("keeps the selected route mounted when a wide detail gains its list pane", () => {
    mockPathname = "/inbox/conversation/0011";

    const view = render(layout());
    expect(mockSlotMounts).toBe(1);

    mockWidth = 1200;
    view.rerender(layout());

    expect(
      view.UNSAFE_getByProps({ accessibilityLabel: "Inbox list", role: "region" }),
    ).toBeTruthy();
    expect(
      view.UNSAFE_getByProps({ accessibilityLabel: "Inbox detail", role: "region" }),
    ).toBeTruthy();
    expect(view.getByText("Inbox list")).toBeTruthy();
    expect(view.getByText("Selected route")).toBeTruthy();
    expect(mockSlotMounts).toBe(1);
  });

  it("keeps unrelated route families single-surface", () => {
    mockPathname = "/more/settings";
    mockWidth = 1200;

    const view = render(layout());

    expect(view.getByText("Selected route")).toBeTruthy();
    expect(view.queryByText("Inbox list")).toBeNull();
    expect(view.UNSAFE_getByProps({ accessibilityLabel: "Inbox content" })).toBeTruthy();
  });
});
