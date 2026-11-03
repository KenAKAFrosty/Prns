import { Text, Button, View } from "react-native";
import { act, fireEvent, render, screen } from "@testing-library/react-native";
import { prns_chat_app as app } from "@prns/app-bindings";
import * as platform from "@prns/app-platform";
import { clearAppPresentation, markPresentationResetPending } from "../src/runtime/reset-presentation";
import { useInstallationReset } from "../src/runtime/installation-reset";
jest.mock("@prns/app-bindings", () => ({ prns_chat_app: { appPrepareReset: jest.fn(), appReset: jest.fn() } }));
jest.mock("@prns/app-platform", () => ({ installationPresent: jest.fn(), prepareReset: jest.fn(), stop: jest.fn(), resetPreferences: jest.fn() }));
jest.mock("../src/runtime/reset-presentation", () => ({ clearAppPresentation: jest.fn(), markPresentationResetPending: jest.fn() }));
const block = jest.fn();
const finished = jest.fn();
const exact = {} as app.AppResetReviewLike;
function Probe({ pending = false }: { pending?: boolean }) {
  const reset = useInstallationReset(pending, block, finished);
  return <View><Text>{reset.state.error}</Text><Text>{reset.state.localDeleted ? "Native deleted" : "Native present"}</Text>
    <Button title="Review" onPress={() => { void reset.prepare(); }} />
    <Button title="Confirm" onPress={() => { void reset.confirm(); }} />
    <Button title="Cleanup" onPress={() => { void reset.resumeCleanup(); }} /></View>;
}
beforeEach(() => {
  jest.resetAllMocks();
  jest.mocked(app.appPrepareReset).mockReturnValue(exact);
  jest.mocked(platform.prepareReset).mockResolvedValue();
  jest.mocked(platform.stop).mockResolvedValue();
  jest.mocked(platform.resetPreferences).mockResolvedValue();
  jest.mocked(platform.installationPresent).mockResolvedValue(false);
  jest.mocked(app.appReset).mockResolvedValue();
  jest.mocked(markPresentationResetPending).mockResolvedValue();
  jest.mocked(clearAppPresentation).mockResolvedValue();
});

test("review has no deletion; explicit confirmation drains native before clearing presentation", async () => {
  const order: string[] = [];
  jest.mocked(platform.stop).mockImplementation(async () => { order.push("stop"); });
  jest.mocked(app.appReset).mockImplementation(async () => { order.push("native"); });
  jest.mocked(platform.resetPreferences).mockImplementation(async () => { order.push("preferences"); });
  jest.mocked(clearAppPresentation).mockImplementation(async () => { order.push("presentation"); });
  await render(<Probe />);
  await act(async () => { await fireEvent.press(screen.getByText("Review")); });
  expect(app.appReset).not.toHaveBeenCalled();
  expect(platform.prepareReset).toHaveBeenCalledTimes(1);
  await act(async () => { await fireEvent.press(screen.getByText("Confirm")); });
  expect(app.appReset).toHaveBeenCalledWith(exact);
  expect(order).toEqual(["stop", "native", "preferences", "presentation"]);
  expect(finished).toHaveBeenCalledTimes(1);
});

test("an uncertain reset-marker write blocks the UI and never starts native deletion", async () => {
  jest.mocked(markPresentationResetPending).mockRejectedValueOnce(new Error("Marker reply lost"));
  await render(<Probe />);
  await act(async () => { await fireEvent.press(screen.getByText("Review")); });
  await act(async () => { await fireEvent.press(screen.getByText("Confirm")); });
  expect(block).toHaveBeenCalledTimes(1);
  expect(app.appReset).not.toHaveBeenCalled();
  expect(screen.getByText(/Marker reply lost/)).toBeTruthy();
  await act(async () => { await fireEvent.press(screen.getByText("Confirm")); });
  expect(app.appReset).toHaveBeenCalledWith(exact);
});

test("preference cleanup retries without replaying native deletion", async () => {
  jest.mocked(platform.resetPreferences).mockRejectedValueOnce(new Error("Service still releasing"));
  await render(<Probe />);
  await act(async () => { await fireEvent.press(screen.getByText("Review")); });
  await act(async () => { await fireEvent.press(screen.getByText("Confirm")); });
  expect(screen.getByText("Native deleted")).toBeTruthy();
  await act(async () => { await fireEvent.press(screen.getByText("Cleanup")); });
  expect(app.appReset).toHaveBeenCalledTimes(1);
  expect(finished).toHaveBeenCalledTimes(1);
});

test("pending reset after reload requires fresh review when native data remains", async () => {
  jest.mocked(platform.installationPresent).mockResolvedValue(true);
  await render(<Probe pending />);
  expect(app.appReset).not.toHaveBeenCalled();
  expect(clearAppPresentation).not.toHaveBeenCalled();
  expect(finished).not.toHaveBeenCalled();
});

test("pending reset after reload resumes only cleanup when native data is absent", async () => {
  await render(<Probe pending />);
  expect(app.appPrepareReset).not.toHaveBeenCalled();
  expect(app.appReset).not.toHaveBeenCalled();
  expect(clearAppPresentation).toHaveBeenCalledTimes(1);
  expect(finished).toHaveBeenCalledTimes(1);
});
