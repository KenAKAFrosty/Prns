import { Platform } from "react-native";
import { act, fireEvent, render, screen } from "@testing-library/react-native";
import type { prns_service_runtime_uniffi as owners } from "@prns/app-bindings";
import type { NetworkActivity } from "../src/runtime/network-activity";
import { NetworkInspection } from "../src/NetworkInspection";
jest.mock("@prns/app-bindings", () => ({ prns_host_uniffi: {
  InterfaceHealth: { 0: "Online" }, HostPhysicalRole: { 0: "Member" }, HostPhysicalOrigin: { 0: "Discovered" },
} }));
const id = new Uint8Array([1]);
const snapshot = {
  runtime: { onlineInterfaceCount: 1, rxBytes: 0n, txBytes: 0n, rxBps: 0n, txBps: 0n }, activeLinkCount: 0,
  persistence: {}, interfaces: [], destinationIdentities: [],
  routes: [{ destination: id, interfaceId: id, hops: 2, learnedAtMillis: 1000n, lastRouteActivityAtMillis: 4000n, expiresAtMillis: 10_000n }],
};
async function press(name: string) { await act(async () => { await fireEvent.press(screen.getByRole("button", { name })); }); }
async function details(name: string) {
  await press("Network details");
  const dismiss = screen.container.queryAll((value) => typeof value.props.onDismiss === "function")[0]!.props.onDismiss;
  await press(name);
  if (Platform.OS === "ios") await act(async () => { dismiss(); });
}
test("physical peers and route ages come from one native inspection capture", async () => {
  const inspection = jest.fn(async () => ({ host: snapshot, engineTimeMillis: 6000n, interfaces: [{
    runtimeInterface: id, logicalInterface: id, attachmentEpoch: 3n, parent: new Uint8Array([2]),
    role: 0, origin: 0, health: 0, name: "Nearby phone", endpoint: "peer", rxBytes: 50n, txBytes: 10n,
    radio: { tag: "Unavailable" }, details: { tag: "Unavailable" },
  }] }));
  const owner = { host: () => ({ inspection }) } as unknown as owners.ServiceRuntimeLike;
  await render(<NetworkInspection owner={owner} active networkKey="1" />);
  expect(screen.queryByRole("button", { name: "Physical peers" })).toBeNull();
  await details("Physical peers");
  expect(screen.getByText("Nearby phone")).toBeTruthy();
  expect(screen.queryByText("Parent: 02")).toBeNull();
  await press("Physical peer details: Nearby phone");
  expect(screen.getByText("Parent: 02")).toBeTruthy();
  await press("Routes");
  expect(screen.queryByText(/Learned 5s ago/)).toBeNull();
  await press("Route details: 01");
  expect(screen.getByText("Learned 5s ago · last activity 2s ago")).toBeTruthy();
  expect(screen.getByText("Expires in 4s")).toBeTruthy();
  expect(inspection).toHaveBeenCalledTimes(1);
});

test("network restart reborrows the current host instead of polling a stopped generation", async () => {
  const first = jest.fn(async () => ({ host: snapshot, engineTimeMillis: 6000n, interfaces: [] }));
  const second = jest.fn(async () => ({ host: snapshot, engineTimeMillis: 8000n, interfaces: [] }));
  const host = jest.fn().mockReturnValueOnce({ inspection: first }).mockReturnValue({ inspection: second });
  const owner = { host } as unknown as owners.ServiceRuntimeLike;
  const view = await render(<NetworkInspection owner={owner} active networkKey="1" />);
  await view.rerender(<NetworkInspection owner={owner} active networkKey="2" />);
  expect(first).toHaveBeenCalledTimes(1);
  expect(second).toHaveBeenCalledTimes(1);
});

test("zero route activity is unknown while real timestamps keep engine-clock ages", async () => {
  const inspection = jest.fn(async () => ({ host: { ...snapshot,
    routes: [{ ...snapshot.routes[0]!, lastRouteActivityAtMillis: 0n }],
  }, engineTimeMillis: 6000n, interfaces: [] }));
  const owner = { host: () => ({ inspection }) } as unknown as owners.ServiceRuntimeLike;
  await render(<NetworkInspection owner={owner} active networkKey="1" />);
  await press("Routes");
  expect(screen.queryByText(/Learned 5s ago/)).toBeNull();
  await press("Route details: 01");
  expect(screen.getByText("Learned 5s ago · last activity unknown")).toBeTruthy();
  expect(screen.queryByText(/last activity 6s ago/)).toBeNull();
  expect(screen.getByText("Expires in 4s")).toBeTruthy();
});


test("network inspection updates automatically and offers retry only after failure", async () => {
  jest.useFakeTimers();
  const value = { host: snapshot, engineTimeMillis: 6000n, interfaces: [] };
  const inspection = jest.fn().mockResolvedValueOnce(value).mockRejectedValueOnce(new Error("Temporary network read failure")).mockResolvedValue(value);
  const owner = { host: () => ({ inspection }) } as unknown as owners.ServiceRuntimeLike;
  const view = await render(<NetworkInspection owner={owner} active networkKey="1" />);
  expect(screen.queryByText("Refresh network")).toBeNull();
  expect(screen.queryByText("Retry network inspection")).toBeNull();
  await act(async () => { jest.advanceTimersByTime(2000); });
  expect(screen.getByText("Temporary network read failure")).toBeTruthy();
  expect(screen.getByText("1 connections online · 0 active links")).toBeTruthy();
  await act(async () => { await fireEvent.press(screen.getByText("Retry network inspection")); });
  expect(screen.queryByText("Retry network inspection")).toBeNull();
  expect(inspection).toHaveBeenCalledTimes(3);
  await view.unmount();
  jest.useRealTimers();
});


test("counted announcement tab keeps raw payloads separate from verified names and clear is a rare explicit action", async () => {
  const raw = { id: 1n, observedAt: 1000, event: { tag: "AnnounceHeard", inner: {
    destination: new Uint8Array([3]), sourceInterface: id, hops: 2, appData: new TextEncoder().encode("Unverified name"),
  } } };
  const verified = { ...raw, id: 2n, event: { ...raw.event, inner: { ...raw.event.inner, destination: new Uint8Array([4]) } } };
  const activity = { generation: "1", announcements: [raw, verified], evicted: 7, dropped: 3n, error: undefined,
    discovered: [{ observation: verified, contact: { claims: { displayName: "Verified Alex" } } }], discoveryError: undefined } as unknown as NetworkActivity;
  const inspection = jest.fn(async () => ({ host: snapshot, engineTimeMillis: 6000n, interfaces: [] }));
  const owner = { host: () => ({ inspection }) } as unknown as owners.ServiceRuntimeLike;
  const clear = jest.fn();
  await render(<NetworkInspection owner={owner} active networkKey="1" activity={activity} clearActivity={clear} />);
  expect(screen.getAllByText("Network activity")).toHaveLength(1);
  expect(screen.getByRole("button", { name: "Connections" })).toHaveTextContent("Connections (0)");
  expect(screen.getByRole("button", { name: "Routes" })).toHaveTextContent("Routes (1)");
  expect(screen.getByRole("button", { name: "Announcements" })).toHaveTextContent("Announcements (2)");
  await press("Announcements");
  expect(screen.getByText("2 shown · 7 older entries removed · 3 events not captured")).toBeTruthy();
  expect(screen.getByText("Network announcement")).toBeTruthy();
  expect(screen.getByText("Verified Alex")).toBeTruthy();
  expect(screen.queryByText("Unverified name")).toBeNull();
  expect(screen.queryByText(/^Public app data:/)).toBeNull();
  expect(screen.queryByText("Clear displayed announcements")).toBeNull();
  await press("Announcement details: 1");
  expect(screen.getByText("Destination: 03")).toBeTruthy();
  expect(screen.getByText("Source interface: 01")).toBeTruthy();
  expect(screen.getByText("Public app data: 556e7665726966696564206e616d65")).toBeTruthy();
  expect(clear).not.toHaveBeenCalled();
  await details("Clear displayed announcements");
  expect(clear).toHaveBeenCalledTimes(1);
  expect(inspection).toHaveBeenCalledTimes(1);
});
