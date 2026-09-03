import { fireEvent, render, waitFor } from "@testing-library/react-native";
import { Alert } from "react-native";

import { RecoveryScreen } from "./recovery-screen";

const mockInspectDevelopmentIdentity = jest.fn();
const mockResetNativeData = jest.fn();
const mockResetScaffoldData = jest.fn();
const mockReplace = jest.fn();

jest.mock("expo-router", () => ({
  useRouter: () => ({ replace: mockReplace }),
}));

jest.mock("@/native/runtime-provider", () => ({
  runtimeProvider: {
    availability: { type: "available", platform: "ios" },
    runtime: {
      inspectDevelopmentIdentity: () => mockInspectDevelopmentIdentity(),
      resetDevelopmentData: () => mockResetNativeData(),
    },
  },
}));

jest.mock("@/state/scaffold-state-context", () => ({
  useScaffoldState: () => ({ resetDevelopmentData: mockResetScaffoldData }),
}));

describe("development identity recovery", () => {
  beforeEach(() => {
    jest.clearAllMocks();
  });

  it("preserves operational unavailability detail without suggesting reset", async () => {
    mockInspectDevelopmentIdentity.mockResolvedValue({
      type: "unavailable",
      detail: "primary identity directory is temporarily locked",
    });
    const view = render(<RecoveryScreen />);

    await waitFor(() =>
      expect(view.getByText("primary identity directory is temporarily locked")).toBeTruthy(),
    );
    expect(view.getByText("Identity storage unavailable")).toBeTruthy();
    expect(view.queryByRole("button", { name: "Reset app data" })).toBeNull();
    expect(view.getByRole("button", { name: "Retry inspection" })).toBeTruthy();
  });

  it("requires confirmation before resetting malformed development state", async () => {
    mockInspectDevelopmentIdentity.mockResolvedValue({
      type: "developmentResetRequired",
      reason: "primary identity holds 63 bytes instead of 64",
    });
    mockResetNativeData.mockResolvedValue({ type: "alreadyStopped" });
    mockResetScaffoldData.mockResolvedValue(undefined);
    const alert = jest.spyOn(Alert, "alert").mockImplementation((_title, _message, buttons) => {
      buttons?.find(({ text }) => text === "Reset")?.onPress?.();
    });
    const view = render(<RecoveryScreen />);

    await waitFor(() =>
      expect(view.getByText("primary identity holds 63 bytes instead of 64")).toBeTruthy(),
    );
    fireEvent.press(view.getByRole("button", { name: "Reset app data" }));

    await waitFor(() => expect(mockResetNativeData).toHaveBeenCalledTimes(1));
    expect(mockResetScaffoldData).toHaveBeenCalledTimes(1);
    expect(mockReplace).toHaveBeenCalledWith("/onboarding/welcome");
    expect(alert).toHaveBeenCalledWith(
      "Reset app data?",
      "This permanently removes your primary identity, paired-node access, saved contacts, messages, and app preferences from this device.",
      expect.any(Array),
    );
    alert.mockRestore();
  });

  it("shows a rejected inspection and offers retry", async () => {
    mockInspectDevelopmentIdentity.mockRejectedValue(new Error("native inspection disconnected"));
    const view = render(<RecoveryScreen />);

    await waitFor(() => expect(view.getByText("native inspection disconnected")).toBeTruthy());
    expect(view.getByText("Could not check identity")).toBeTruthy();
    expect(view.getByRole("button", { name: "Retry inspection" })).toBeTruthy();
  });
});
