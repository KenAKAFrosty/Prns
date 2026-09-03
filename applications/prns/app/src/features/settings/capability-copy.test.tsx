import { fireEvent, render } from "@testing-library/react-native";
import type { ReactNode } from "react";
import { Alert } from "react-native";

import { MoreScreen } from "@/features/more/more-screen";
import { AboutScreen } from "./about-screen";
import { HelpScreen } from "./help-screen";
import { SettingsScreen } from "./settings-screen";

const mockReplace = jest.fn();
const mockResetNativeData = jest.fn();
const mockResetScaffoldData = jest.fn();
const mockUpdateScaffoldState = jest.fn();

jest.mock("expo-router", () => ({
  Link: ({ children }: { readonly children: ReactNode }) => children,
  useRouter: () => ({ replace: mockReplace }),
}));

jest.mock("@/native/runtime-provider", () => ({
  runtimeProvider: {
    availability: { type: "available", platform: "ios" },
    runtime: { resetDevelopmentData: () => mockResetNativeData() },
  },
}));

jest.mock("@/state/scaffold-state-context", () => ({
  useScaffoldState: () => ({
    resetDevelopmentData: mockResetScaffoldData,
    state: { scaffoldSchema: 2, showUnavailableFeatures: true },
    updateScaffoldState: mockUpdateScaffoldState,
  }),
}));

describe("development capability copy", () => {
  beforeEach(() => {
    jest.clearAllMocks();
  });

  it("names native contacts and the durable mailbox without claiming placeholders work", () => {
    const about = render(<AboutScreen />);
    expect(
      about.getByText(
        /a native saved-contact directory, and a durable direct-LXMF Inbox and Outbox/u,
      ),
    ).toBeTruthy();
    expect(about.queryByText(/Mailbox and contact services are not present/u)).toBeNull();
    about.unmount();

    const help = render(<HelpScreen />);
    expect(
      help.getByText(/native saved contacts, a durable direct-LXMF Inbox and Outbox/u),
    ).toBeTruthy();
    expect(
      help.getByText(
        /dedicated message inspector, contact merging, separate identity and interface views/u,
      ),
    ).toBeTruthy();
    expect(help.queryByText(/^Messages, contacts,/u)).toBeNull();
  });

  it("labels the unavailable More destinations as placeholders", () => {
    const view = render(<MoreScreen />);

    expect(
      view.getByText(
        "Open implemented Settings and Help here. Identities, Interfaces, Notifications, and Activity remain clearly labelled placeholders when unavailable features are shown.",
      ),
    ).toBeTruthy();
  });

  it("discloses every durable data category removed by development reset", () => {
    const alert = jest.spyOn(Alert, "alert").mockImplementation();
    const view = render(<SettingsScreen />);

    fireEvent.press(view.getByRole("button", { name: "Reset development data" }));

    expect(alert).toHaveBeenCalledWith(
      "Reset development data?",
      "This stops the local node and permanently removes local preview preferences, the primary and Bluetooth identities, RemoteControl identities and authorization state, saved contacts, and all Inbox and Outbox messages from this development install.",
      expect.any(Array),
    );
    alert.mockRestore();
  });
});
