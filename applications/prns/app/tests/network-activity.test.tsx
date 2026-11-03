import { Button, Text, View } from "react-native";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react-native";
import type { prns_service_runtime_uniffi as owners } from "@prns/app-bindings";
import { useNetworkActivity } from "../src/runtime/network-activity";
jest.mock("@prns/app-bindings", () => ({ prns_contact_card: {}, prns_host_uniffi: {
  HostEvent_Tags: { Diagnostic: "Diagnostic" }, DiagnosticEvent_Tags: { AnnounceHeard: "AnnounceHeard", DiagnosticsDropped: "DiagnosticsDropped" },
} }));
test("announcement observation stays bounded and reports native stream loss", async () => {
  const announcements = Array.from({ length: 250 }, (_, index) => ({ tag: "Diagnostic", inner: { event: {
    tag: "AnnounceHeard", inner: { destination: new Uint8Array([index]), sourceInterface: new Uint8Array([1]), hops: 1, appData: new Uint8Array() },
  } } }));
  const events: unknown[] = [...announcements, { tag: "Diagnostic", inner: { event: {
    tag: "DiagnosticsDropped", inner: { count: { high: 1n, low: 7n } },
  } } }];
  const close = jest.fn();
  const stream = { ready: async () => events.length > 0, tryNext: () => events.shift(), closeStream: close };
  const owner = { host: () => ({ diagnostics: () => stream }) } as unknown as owners.ServiceRuntimeLike;
  function Probe() {
    const { activity, clearRawEvents } = useNetworkActivity(owner, "1");
    return <View><Text>{activity.announcements.length} shown, {activity.evicted} older, {activity.dropped.toString()} dropped</Text>
      <Button title="Clear" onPress={clearRawEvents} /></View>;
  }
  await render(<Probe />);
  await waitFor(() => expect(screen.getByText("200 shown, 50 older, 18446744073709551623 dropped")).toBeTruthy());
  await waitFor(() => expect(close).toHaveBeenCalledTimes(1));
  await act(async () => { await fireEvent.press(screen.getByText("Clear")); });
  expect(screen.getByText("0 shown, 0 older, 18446744073709551623 dropped")).toBeTruthy();
});

test("verified discovery survives consumer remounts and raw clearing, while discovery clearing and network replacement fence pending key lookups", async () => {
  mockContactCard();
  let accept!: (key: Uint8Array) => void;
  const pending = new Promise<Uint8Array>((resolve) => { accept = resolve; });
  const event = { tag: "Diagnostic", inner: { event: { tag: "AnnounceHeard", inner: {
    destination: new Uint8Array([4]), sourceInterface: new Uint8Array([1]), hops: 1, appData: new Uint8Array(148),
  } } } };
  const queue = [event];
  const client = { diagnostics: () => ({ ready: async () => queue.length > 0, tryNext: () => queue.shift(), closeStream: jest.fn() }),
    destinationPublicKey: jest.fn(() => pending) };
  const owner = { host: () => client } as unknown as owners.ServiceRuntimeLike;
  function Probe({ generation, visible }: { generation: string | undefined; visible: boolean }) {
    const { activity, clearRawEvents, clearDiscoveries } = useNetworkActivity(owner, generation);
    return <View><Text>{activity.discovered.length} contacts</Text>{visible && <Text>{activity.discovered[0]?.contact.claims.displayName ?? "No people"}</Text>}
      <Text>{activity.announcements.length} announcements</Text>
      <Button title="Clear network log" onPress={clearRawEvents} />
      <Button title="Clear discoveries" onPress={clearDiscoveries} /></View>;
  }
  const view = await render(<Probe generation="1" visible />);
  await waitFor(() => expect(client.destinationPublicKey).toHaveBeenCalledTimes(1));
  await act(async () => accept(new Uint8Array([3])));
  await screen.findByText("Alice");
  await view.rerender(<Probe generation="1" visible={false} />);
  await view.rerender(<Probe generation="1" visible />);
  expect(screen.getByText("Alice")).toBeTruthy();
  expect(screen.getByText("1 announcements")).toBeTruthy();
  await fireEvent.press(screen.getByText("Clear network log"));
  expect(screen.getByText("0 announcements")).toBeTruthy();
  expect(screen.getByText("Alice")).toBeTruthy();
  await view.rerender(<Probe generation={undefined} visible />);
  expect(screen.getByText("0 contacts")).toBeTruthy();

  queue.push(event); client.destinationPublicKey.mockImplementation(() => new Promise((resolve) => { accept = resolve; }));
  await view.rerender(<Probe generation="2" visible />);
  await waitFor(() => expect(client.destinationPublicKey).toHaveBeenCalledTimes(2));
  expect(screen.getByText("1 announcements")).toBeTruthy();
  await fireEvent.press(screen.getByText("Clear discoveries"));
  await act(async () => accept(new Uint8Array([3])));
  expect(screen.getByText("0 contacts")).toBeTruthy();
  expect(screen.getByText("1 announcements")).toBeTruthy();

  queue.push(event);
  await view.rerender(<Probe generation="3" visible />);
  await waitFor(() => expect(client.destinationPublicKey).toHaveBeenCalledTimes(3));
  await view.rerender(<Probe generation="4" visible />);
  await act(async () => accept(new Uint8Array([3])));
  expect(screen.getByText("0 contacts")).toBeTruthy();
});


function mockContactCard() {
  const { prns_contact_card: cards } = require("@prns/app-bindings");
  cards.serviceNameHash = () => new Uint8Array([7]).buffer;
  cards.verifyContactCard = () => ({ publicIdentity: new Uint8Array([3]).buffer, identityHash: new Uint8Array([3]).buffer,
    serviceNameHash: new Uint8Array([7]).buffer, destinationHash: new Uint8Array([4]).buffer, displayName: "Alice" });
  cards.encodeContactCardUri = () => "card:3";
}

test.each(["queued raw batch", "pending verification"] as const)("clearing a %s leaves the same contact verification eligible", async (phase) => {
  mockContactCard();
  let accept!: (key: Uint8Array) => void;
  const pending = new Promise<Uint8Array>((resolve) => { accept = resolve; });
  const queue = [{ tag: "Diagnostic", inner: { event: { tag: "AnnounceHeard", inner: {
    destination: new Uint8Array([4]), sourceInterface: new Uint8Array([1]), hops: 1, appData: new Uint8Array(148),
  } } } }];
  let clearRawEvents!: () => void;
  let cleared = false;
  const client = {
    diagnostics: () => ({
      ready: async () => queue.length > 0,
      tryNext: () => {
        const next = queue.shift();
        if (next === undefined && phase === "queued raw batch" && !cleared) {
          // Clear after the stream yielded a row but before its queued projection commits.
          cleared = true;
          clearRawEvents();
        }
        return next;
      },
      closeStream: jest.fn(),
    }),
    destinationPublicKey: jest.fn(() => pending),
  };
  const owner = { host: () => client } as unknown as owners.ServiceRuntimeLike;
  function Probe() {
    const observations = useNetworkActivity(owner, "1");
    clearRawEvents = observations.clearRawEvents;
    return <View><Text>{observations.activity.announcements.length} announcements</Text>
      <Text>{observations.activity.discovered.length} contacts</Text>
      <Button title="Clear network log" onPress={clearRawEvents} /></View>;
  }
  await render(<Probe />);
  await waitFor(() => expect(client.destinationPublicKey).toHaveBeenCalledTimes(1));
  if (phase === "pending verification") await fireEvent.press(screen.getByText("Clear network log"));
  expect(screen.getByText("0 announcements")).toBeTruthy();
  await act(async () => accept(new Uint8Array([3])));
  await screen.findByText("1 contacts");
  expect(screen.getByText("0 announcements")).toBeTruthy();
});


test("replacing a native owner wrapper releases diagnostics before the previous read finishes", async () => {
  let claimed = false;
  const pending: Array<(ready: boolean) => void> = [];
  const streams: Array<{ closeStream: jest.Mock; uniffiDestroy: jest.Mock }> = [];
  const event = { tag: "Diagnostic", inner: { event: { tag: "AnnounceHeard", inner: {
    destination: new Uint8Array([4]), sourceInterface: new Uint8Array([1]), hops: 1, appData: new Uint8Array(),
  } } } };
  const diagnostics = jest.fn(() => {
    if (claimed) throw new Error("BindingError.AlreadyClaimed");
    claimed = true;
    const queue = streams.length === 0 ? [] : [event];
    // Cancellation can finish later than React's cleanup and replacement effect.
    const waiting = new Promise<boolean>((resolve) => { pending.push(resolve); });
    const stream = {
      ready: () => queue.length > 0 ? Promise.resolve(true) : waiting,
      tryNext: () => queue.shift(),
      closeStream: jest.fn(() => { claimed = false; }),
      uniffiDestroy: jest.fn(),
    };
    streams.push(stream);
    return stream;
  });
  const client = { diagnostics };
  const wrapper = () => ({ host: () => client }) as unknown as owners.ServiceRuntimeLike;
  function Probe({ owner }: { owner: owners.ServiceRuntimeLike }) {
    const { activity } = useNetworkActivity(owner, "1");
    return <View><Text>{activity.error ?? "Observing"}</Text><Text>{activity.announcements.length} announcements</Text></View>;
  }
  const view = await render(<Probe owner={wrapper()} />);
  expect(diagnostics).toHaveBeenCalledTimes(1);
  await view.rerender(<Probe owner={wrapper()} />);
  expect(streams[0]!.closeStream).toHaveBeenCalledTimes(1);
  expect(streams[0]!.uniffiDestroy).not.toHaveBeenCalled();
  expect(diagnostics).toHaveBeenCalledTimes(2);
  await screen.findByText("1 announcements");
  expect(screen.getByText("Observing")).toBeTruthy();

  await act(async () => { pending[0]!(false); });
  expect(streams[0]!.closeStream).toHaveBeenCalledTimes(1);
  expect(streams[0]!.uniffiDestroy).toHaveBeenCalledTimes(1);
  expect(claimed).toBe(true);
  expect(streams[1]!.closeStream).not.toHaveBeenCalled();
  await view.unmount();
  expect(streams[1]!.closeStream).toHaveBeenCalledTimes(1);
  await act(async () => { pending[1]!(false); });
  expect(streams[1]!.closeStream).toHaveBeenCalledTimes(1);
  await waitFor(() => expect(streams[1]!.uniffiDestroy).toHaveBeenCalledTimes(1));
  expect(claimed).toBe(false);
});
