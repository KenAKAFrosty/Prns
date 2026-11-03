import { act, fireEvent, render } from "@testing-library/react-native";
import { prns_host_uniffi as rc } from "@prns/app-bindings";
import { Wifi } from "../src/nodes/Wifi";
import { Facts } from "../src/nodes/presentation";

jest.mock("@prns/app-bindings", () => {
  const tags = new Proxy({}, { get: (_, key) => key });
  const variants = new Proxy({}, { get: (_, key) => class { tag = key; inner: unknown; constructor(value: unknown) { this.inner = value; } } });
  return { prns_host_uniffi: { RemoteControlRequest: variants, RemoteControlWifiTransactionStatus_Tags: tags,
    RemoteControlRequestKind: { InspectWifiTransaction: 0, StageWifiCredentials: 1, ActivateWifiCredentials: 2, ConfirmWifiCredentials: 3, CancelWifiCredentials: 4 } } };
});
const supported = { kinds: [0, 1, 2, 3, 4] };
test("Wi-Fi activation and confirmation use only the exact observed native revision", async () => {
  const review = jest.fn(); const refresh = jest.fn(); const revision: rc.RemoteControlWifiCredentialRevision = { value: 0xffff_ffff };
  const staged = { tag: "Staged", inner: { revision } } as unknown as rc.RemoteControlWifiTransactionStatus;
  const screen = await render(<Wifi supported={supported} status={staged} disabled={false} blocked={false} review={review} refresh={refresh} />);
  await act(async () => await fireEvent.press(screen.getByText("Review activating staged Wi-Fi revision")));
  expect(review).toHaveBeenCalledWith(expect.objectContaining({ tag: "ActivateWifiCredentials", inner: { revision } }));
  expect(screen.queryByText("Review confirming working Wi-Fi revision")).toBeNull();
  await screen.rerender(<Wifi supported={supported} status={{ tag: "AwaitingConfirmation", inner: { revision, remaining: { value: 10 } } } as unknown as rc.RemoteControlWifiTransactionStatus} disabled={false} blocked={false} review={review} refresh={refresh} />);
  await act(async () => await fireEvent.press(screen.getByText("Review confirming working Wi-Fi revision")));
  expect(review).toHaveBeenLastCalledWith(expect.objectContaining({ tag: "ConfirmWifiCredentials", inner: { revision } }));
});

test("an uncertain change blocks Wi-Fi mutations but still allows inspection; review hides passwords", async () => {
  const review = jest.fn(); const refresh = jest.fn();
  const screen = await render(<Wifi supported={supported} status={undefined} disabled={false} blocked review={review} refresh={refresh} />);
  await fireEvent.changeText(screen.getByLabelText("Wi-Fi network name"), "home");
  await fireEvent.changeText(screen.getByLabelText("Wi-Fi password"), "private-password");
  await act(async () => await fireEvent.press(screen.getByText("Review staged Wi-Fi credentials")));
  expect(review).not.toHaveBeenCalled();
  await act(async () => await fireEvent.press(screen.getByText("Read Wi-Fi setup status")));
  expect(refresh).toHaveBeenCalledTimes(1);
  await screen.unmount();
  const facts = await render(<Facts value={new rc.RemoteControlRequest.StageWifiCredentials({ station: { ssid: "home", password: "private-password" } })} />);
  expect(facts.queryByText("private-password")).toBeNull(); expect(facts.getByText("Hidden")).toBeTruthy();
});
