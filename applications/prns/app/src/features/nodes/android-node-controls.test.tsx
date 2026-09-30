import { act, fireEvent, render, waitFor } from "@testing-library/react-native";
import { Linking } from "react-native";

import { AndroidNodeControls } from "./android-node-controls";

const mockRequest = jest.fn(async () => {});
const mockStop = jest.fn(async () => {});
const mockPlatform = {
  status: { connectionNotification: "enabled" },
  failure: null,
  requestConnectionNotificationPermission: mockRequest,
};
const mockRuntime = {
  androidRuntime: mockPlatform as typeof mockPlatform | null,
  canStopNode: true,
  stoppingNode: false,
  stopFailure: null,
  stopNode: mockStop,
};

jest.mock("@/native/development-runtime-context", () => ({
  useDevelopmentRuntime: () => mockRuntime,
}));

beforeEach(() => {
  jest.clearAllMocks();
  mockRuntime.androidRuntime = mockPlatform;
  mockPlatform.status = { connectionNotification: "enabled" };
});

test("has an independent Stop control and never requests a notification on mount", () => {
  const view = render(<AndroidNodeControls />);
  expect(view.getByRole("button", { name: "Stop node" })).toBeEnabled();
  expect(view.queryByRole("button", { name: "Allow node notification" })).toBeNull();
  expect(mockRequest).not.toHaveBeenCalled();
  fireEvent.press(view.getByRole("button", { name: "Stop node" }));
  expect(mockStop).toHaveBeenCalledTimes(1);
});

test.each(["notRequested", "denied"])(
  "requests notification permission only explicitly for %s",
  async (status) => {
    mockPlatform.status.connectionNotification = status;
    let finish: (() => void) | undefined;
    mockRequest.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve;
        }),
    );
    const view = render(<AndroidNodeControls />);
    expect(mockRequest).not.toHaveBeenCalled();
    expect(view.getByText(/This does not enable message alerts/u)).toBeTruthy();
    fireEvent.press(view.getByRole("button", { name: "Allow node notification" }));
    expect(view.getByRole("button", { name: "Please wait…" })).toBeDisabled();
    fireEvent.press(view.getByRole("button", { name: "Please wait…" }));
    await act(async () => finish?.());
    expect(mockRequest).toHaveBeenCalledTimes(1);
    expect(mockStop).not.toHaveBeenCalled();
  },
);

test("offers Settings for blocked app or channel notifications while leaving Stop usable", async () => {
  mockPlatform.status.connectionNotification = "blocked";
  const settings = jest.spyOn(Linking, "openSettings").mockResolvedValue();
  const view = render(<AndroidNodeControls />);
  expect(view.getByRole("button", { name: "Stop node" })).toBeEnabled();
  fireEvent.press(view.getByRole("button", { name: "Open notification settings" }));
  await waitFor(() => expect(settings).toHaveBeenCalledTimes(1));
  expect(mockRequest).not.toHaveBeenCalled();
  settings.mockRestore();
});

test("does not add Android controls to iOS", () => {
  mockRuntime.androidRuntime = null;
  const view = render(<AndroidNodeControls />);
  expect(view.toJSON()).toBeNull();
});
