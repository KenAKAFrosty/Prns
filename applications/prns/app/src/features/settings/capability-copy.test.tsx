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

describe("preview capability copy", () => {
  beforeEach(() => {
    jest.clearAllMocks();
  });

  it("describes available features without implementation or board jargon", () => {
    const about = render(<AboutScreen />);
    expect(
      about.getByText(/pair and check nearby nodes, save contacts, exchange direct messages/u),
    ).toBeTruthy();
    expect(JSON.stringify(about.toJSON())).not.toMatch(
      /E290|signed availability|upstream RemoteControl/iu,
    );
    about.unmount();

    const help = render(<HelpScreen />);
    expect(
      help.getByText(/pair and check nearby nodes, save contacts, exchange direct messages/u),
    ).toBeTruthy();
    expect(
      help.getByText(/Message details, contact merging, identity and connection management/u),
    ).toBeTruthy();
    expect(JSON.stringify(help.toJSON())).not.toMatch(
      /E290|signed availability|upstream RemoteControl/iu,
    );
  });

  it("introduces More without narrating scaffold state", () => {
    const view = render(<MoreScreen />);

    expect(
      view.getByText("Manage identities, connections, notifications, settings, and help."),
    ).toBeTruthy();
  });

  it("discloses every durable data category removed by development reset", () => {
    const alert = jest.spyOn(Alert, "alert").mockImplementation();
    const view = render(<SettingsScreen />);

    fireEvent.press(view.getByRole("button", { name: "Reset app data" }));

    expect(alert).toHaveBeenCalledWith(
      "Reset app data?",
      "This permanently removes your primary identity, paired-node access, saved contacts, messages, and app preferences from this device.",
      expect.any(Array),
    );
    expect(JSON.stringify(view.toJSON())).not.toMatch(
      /E290|signed availability|upstream RemoteControl/iu,
    );
    alert.mockRestore();
  });
});
