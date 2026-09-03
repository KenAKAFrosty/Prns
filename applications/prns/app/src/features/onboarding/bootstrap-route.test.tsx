import { render, waitFor } from "@testing-library/react-native";

import BootstrapRoute from "../../../app/index";

const mockInspectDevelopmentIdentity = jest.fn();
const mockReplace = jest.fn();

jest.mock("expo-router", () => ({
  useRouter: () => ({ replace: mockReplace }),
}));

jest.mock("@/native/runtime-provider", () => ({
  runtimeProvider: {
    availability: { type: "available", platform: "ios" },
    runtime: {
      inspectDevelopmentIdentity: () => mockInspectDevelopmentIdentity(),
    },
  },
}));

jest.mock("@/state/scaffold-state-context", () => ({
  useScaffoldState: () => ({ status: "ready" }),
}));

describe("bootstrap identity routing", () => {
  beforeEach(() => {
    jest.clearAllMocks();
  });

  it.each([
    [{ type: "missing" }, "/onboarding/welcome"],
    [{ type: "present", identityHash: new Uint8Array(16) }, "/nodes"],
    [{ type: "unavailable", detail: "identity directory is locked" }, "/recovery"],
    [{ type: "developmentResetRequired", reason: "primary identity has 63 bytes" }, "/recovery"],
  ] as const)("routes %s without collapsing identity state", async (identity, destination) => {
    mockInspectDevelopmentIdentity.mockResolvedValue(identity);
    const view = render(<BootstrapRoute />);

    await waitFor(() => expect(mockReplace).toHaveBeenCalledWith(destination));
    view.unmount();
  });

  it("routes bridge inspection failure to visible recovery", async () => {
    mockInspectDevelopmentIdentity.mockRejectedValue(new Error("native bridge detached"));
    const view = render(<BootstrapRoute />);

    await waitFor(() => expect(mockReplace).toHaveBeenCalledWith("/recovery"));
    view.unmount();
  });
});
