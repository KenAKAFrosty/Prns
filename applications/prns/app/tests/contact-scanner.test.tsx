import { StrictMode } from "react";
import type { ReactElement } from "react";
import { Linking, View } from "react-native";
import { act, fireEvent, render, screen } from "@testing-library/react-native";
import { CameraView, useCameraPermissions } from "expo-camera";
import type { CameraViewProps, PermissionResponse } from "expo-camera";
import { ContactScanner } from "../src/ContactScanner";

jest.mock("expo-camera", () => ({ CameraView: jest.fn(), useCameraPermissions: jest.fn() }));
const permissions = jest.mocked(useCameraPermissions);
const camera = CameraView as unknown as jest.MockedFunction<(props: CameraViewProps) => ReactElement>;
const request = jest.fn<Promise<PermissionResponse>, []>();
const allowed = { granted: true, canAskAgain: true, status: "granted", expires: "never" } as PermissionResponse;
const denied = { ...allowed, granted: false, status: "denied" } as PermissionResponse;
const scanned = { type: "qr", data: "signed-card" };
const callbacks = () => ({ onScan: jest.fn(), onFailure: jest.fn(), onCancel: jest.fn() });
function permission(value: PermissionResponse | null) {
  permissions.mockReturnValue([value, request, jest.fn()] as ReturnType<typeof useCameraPermissions>);
}
function cameraProps() { return camera.mock.calls.at(-1)![0] as CameraViewProps; }

beforeEach(() => {
  jest.resetAllMocks();
  camera.mockImplementation((props) => <View accessibilityLabel={props.accessibilityLabel} />);
  request.mockResolvedValue(allowed); permission(allowed);
});

test("reading permission never asks automatically; denied permission has an explicit request and paste fallback", async () => {
  permission(null); const props = callbacks(); const view = await render(<ContactScanner {...props} />);
  expect(screen.getByLabelText("Checking camera permission")).toBeTruthy();
  expect(request).not.toHaveBeenCalled(); expect(camera).not.toHaveBeenCalled();
  permission(denied); await view.rerender(<ContactScanner {...props} />);
  request.mockResolvedValue(denied);
  await fireEvent.press(screen.getByText("Allow camera for scanning"));
  await screen.findByText("Camera access was not allowed. You can still paste a contact link.");
  expect(request).toHaveBeenCalledTimes(1); expect(camera).not.toHaveBeenCalled();
});

test("blocked access offers settings and reports settings errors without requesting again", async () => {
  permission({ ...denied, canAskAgain: false });
  const settings = jest.spyOn(Linking, "openSettings").mockRejectedValue(new Error("Unavailable"));
  await render(<ContactScanner {...callbacks()} />);
  await fireEvent.press(screen.getByText("Open camera settings"));
  await screen.findByText(/Settings could not be opened/);
  expect(request).not.toHaveBeenCalled(); settings.mockRestore();
});

test("permission failures leave a visible retry and no camera", async () => {
  permission(denied); request.mockRejectedValue(new Error("Permission service failed"));
  await render(<ContactScanner {...callbacks()} />);
  await fireEvent.press(screen.getByText("Allow camera for scanning"));
  await screen.findByText(/Camera permission could not be checked/);
  expect(screen.getByText("Allow camera for scanning")).toBeTruthy(); expect(camera).not.toHaveBeenCalled();
});

test("the first QR result wins, other formats and repeated frames cannot replace its review", async () => {
  const props = callbacks(); await render(<ContactScanner {...props} />);
  const handler = cameraProps().onBarcodeScanned!;
  await act(() => { handler({ ...scanned, type: "ean13" } as never); handler(scanned as never); handler({ ...scanned, data: "second-card" } as never); });
  expect(props.onScan).toHaveBeenCalledTimes(1); expect(props.onScan).toHaveBeenCalledWith("signed-card");
  expect(cameraProps().barcodeScannerSettings).toEqual({ barcodeTypes: ["qr"] });
});

test("cancelled or unmounted scans ignore already queued camera callbacks", async () => {
  const props = callbacks(); const view = await render(<ContactScanner {...props} />);
  const handler = cameraProps().onBarcodeScanned!;
  await fireEvent.press(screen.getByText("Cancel scan"));
  await act(() => handler(scanned as never));
  expect(props.onCancel).toHaveBeenCalledTimes(1); expect(props.onScan).not.toHaveBeenCalled();
  await view.unmount(); await act(() => handler(scanned as never)); expect(props.onScan).not.toHaveBeenCalled();
});

test("a permission response after unmount cannot create a camera or invoke a result", async () => {
  permission(denied);
  let resolve!: (value: PermissionResponse) => void;
  request.mockImplementation(() => new Promise((done) => { resolve = done; }));
  const props = callbacks(); const view = await render(<ContactScanner {...props} />);
  await fireEvent.press(screen.getByText("Allow camera for scanning")); await view.unmount();
  await act(async () => resolve(allowed));
  expect(camera).not.toHaveBeenCalled(); expect(props.onScan).not.toHaveBeenCalled(); expect(props.onFailure).not.toHaveBeenCalled();
});

test("camera failure consumes the attempt once and offers the text fallback", async () => {
  const props = callbacks(); await render(<ContactScanner {...props} />);
  const cameraProps_ = cameraProps();
  await act(() => { cameraProps_.onMountError!({ message: "Camera busy" }); cameraProps_.onBarcodeScanned!(scanned as never); });
  expect(props.onFailure).toHaveBeenCalledWith(expect.stringContaining("paste a contact link"));
  expect(props.onScan).not.toHaveBeenCalled();
});

test("development effect replay does not permanently disable a live scanner", async () => {
  const props = callbacks(); await render(<StrictMode><ContactScanner {...props} /></StrictMode>);
  await act(() => cameraProps().onBarcodeScanned!(scanned as never)); expect(props.onScan).toHaveBeenCalledTimes(1);
});
