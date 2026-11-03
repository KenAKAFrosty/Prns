import { act, cleanup, fireEvent, render, screen } from "@testing-library/react-native";
import { Connections } from "../../src/connections/Connections";
import { prns_chat_app as app, prns_host_uniffi as host } from "@prns/app-bindings";
import type { prns_service_runtime_uniffi as owners } from "@prns/app-bindings";
import { sameValue } from "../../src/connections/values";

jest.mock("@prns/app-bindings", () => {
  class Auto { readonly tag = "Auto"; }
  class TcpClient {
    readonly tag = "TcpClient";
    readonly inner: { target: string; bitrate: unknown };
    constructor(value: { target: string; bitrate: unknown }) { this.inner = value; }
    static instanceOf(value: { tag: string }) { return value.tag === "TcpClient"; }
  }
  class AutoLan {
    readonly tag = "AutoLan";
    readonly inner: { devices: string[]; ignoredDevices: string[] };
    constructor(value: { devices: string[]; ignoredDevices: string[] }) { this.inner = value; }
    static instanceOf(value: { tag: string }) { return value.tag === "AutoLan"; }
  }
  return { prns_chat_app: { appConnectionProfiles: jest.fn() }, prns_host_uniffi: {
    Bitrate: { Auto }, InterfaceConfig: { TcpClient, AutoLan }, newConnectionProfileId: jest.fn(),
    connectionTcpClientConfigFromTarget: jest.fn((target: string, bitrate: unknown) => new TcpClient({ target, bitrate })),
    ConnectionBindingPhase: { Active: 0, Fenced: 1, Closing: 2, Closed: 3, 0: "Active", 1: "Fenced", 2: "Closing", 3: "Closed" },
    ConnectionAttachmentState: { Attaching: 0, Attached: 1, Retiring: 2, Failed: 3, 0: "Attaching", 1: "Attached", 2: "Retiring", 3: "Failed" },
    InterfaceHealth: { Connected: 0, Disconnected: 1, 0: "Connected", 1: "Disconnected" },
  } };
});
function id(value: number) { const bytes = new Uint8Array(16); bytes[0] = value; return bytes; }
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((yes) => { resolve = yes; }); return { promise, resolve }; }
function fixture() {
  const lan: host.ConnectionProfile = { id: id(1), label: "Local network", enabled: false, config: new host.InterfaceConfig.AutoLan({ devices: [], ignoredDevices: [] }), routing: undefined };
  const tcp: host.ConnectionProfile = { id: id(2), label: "Public node", enabled: true, config: new host.InterfaceConfig.TcpClient({ target: "node.example:4242", bitrate: new host.Bitrate.Auto() }), routing: undefined };
  let saved: host.ConnectionSnapshot = { guard: { incarnation: id(10), revision: 1n }, limits: { profiles: 16, encodedBytes: 65536 }, profiles: [lan, tcp] };
  const client = { snapshot: jest.fn(async () => saved), replace: jest.fn(async (expected: host.ConnectionGuard, profiles: host.ConnectionProfile[]) => {
    expect(expected).toEqual(saved.guard); saved = { ...saved, guard: { ...saved.guard, revision: saved.guard.revision + 1n }, profiles }; return saved;
  }) };
  const binding = { status: jest.fn(() => ({ generation: 1n, phase: host.ConnectionBindingPhase.Active })),
    snapshot: jest.fn(async () => ({ generation: 1n, saved: saved.guard, appliedRevision: saved.guard.revision, phase: host.ConnectionBindingPhase.Active, attachments: [] })),
    retry: jest.fn(async (_guard: host.ConnectionGuard, _id: host.ConnectionProfile["id"]): Promise<void> => undefined) };
  const network = { snapshot: jest.fn(async () => ({ interfaces: [] })) };
  const owner = { connectionBinding: () => binding, host: () => network } as unknown as owners.ServiceRuntimeLike;
  jest.mocked(app.appConnectionProfiles).mockReturnValue(client as unknown as host.ConnectionProfileClient);
  jest.mocked(host.newConnectionProfileId).mockReturnValue(id(3));
  return { lan, tcp, client, binding, network, owner, get saved() { return saved; }, set saved(value: host.ConnectionSnapshot) { saved = value; } };
}
async function press(name: string) { await act(async () => { await fireEvent.press(screen.getByText(name)); }); }
async function tick() { await act(async () => { jest.advanceTimersByTime(2000); }); }
async function enter(name: string, target: string) { await fireEvent.changeText(screen.getByLabelText("Connection name"), name); await fireEvent.changeText(screen.getByLabelText("Server address"), target); }
beforeEach(() => { jest.useFakeTimers(); jest.clearAllMocks(); });
afterEach(async () => { await cleanup(); jest.useRealTimers(); });

test("saved LAN and TCP edits use exact guards while the host is absent", async () => {
  const data = fixture(); await render(<Connections active />);
  await act(async () => { await fireEvent(screen.getByLabelText("Enable Local network"), "valueChange", true); });
  expect(data.client.replace).toHaveBeenLastCalledWith(expect.objectContaining({ revision: 1n }), [expect.objectContaining({ enabled: true }), data.tcp]);
  await press("Add TCP server"); await enter("My server", "[::1]:4242"); await press("Save server");
  expect(host.connectionTcpClientConfigFromTarget).toHaveBeenCalledWith("[::1]:4242", expect.objectContaining({ tag: "Auto" }));
  expect(host.newConnectionProfileId).toHaveBeenCalledTimes(1);
  expect(data.client.replace).toHaveBeenLastCalledWith(expect.objectContaining({ revision: 2n }), expect.arrayContaining([expect.objectContaining({ id: id(3), label: "My server" })]));
  expect(screen.getByText("Saved connections will apply when networking starts.")).toBeTruthy();
});
test("conflict refresh keeps the draft and requires review before a fresh guarded save", async () => {
  const data = fixture(); await render(<Connections active />);
  await press("Edit Public node"); await enter("My draft", "other.example:9000"); const previousGuard = data.saved.guard;
  data.client.replace.mockImplementationOnce(async () => {
    data.saved = { ...data.saved, guard: { ...data.saved.guard, revision: 2n }, profiles: [{ ...data.lan, enabled: true }, { ...data.tcp, label: "Changed elsewhere" }] }; throw new Error("Settings changed elsewhere");
  });
  await press("Save server"); expect(data.client.replace).toHaveBeenCalledWith(previousGuard, expect.any(Array));
  expect(screen.getByLabelText("Connection name")).toHaveDisplayValue("My draft"); expect(screen.getByText("Save server")).toBeDisabled(); expect(data.client.replace).toHaveBeenCalledTimes(1);
  await press("Review current settings"); await press("Save server");
  expect(data.client.replace).toHaveBeenLastCalledWith(expect.objectContaining({ revision: 2n }), [expect.objectContaining({ enabled: true }), expect.objectContaining({ label: "My draft" })]);
});
test("lost committed reply is reconciled from exact canonical values without replay or a new ID", async () => {
  const data = fixture(); await render(<Connections active />); await press("Add TCP server"); await enter("Saved despite lost reply", "new.example:9000");
  data.client.replace.mockImplementationOnce(async (_guard, profiles) => { data.saved = { ...data.saved, guard: { ...data.saved.guard, revision: 2n }, profiles }; throw new Error("Reply lost"); });
  await press("Save server"); expect(data.client.replace).toHaveBeenCalledTimes(1); expect(host.newConnectionProfileId).toHaveBeenCalledTimes(1);
  expect(screen.getByText("These settings are saved. No repeat save was sent.")).toBeTruthy(); expect(screen.queryByLabelText("Connection name")).toBeNull();
});
test("unavailable reconciliation blocks another write and preserves the exact pending proposal", async () => {
  const data = fixture(); await render(<Connections active />); await press("Edit Public node"); await enter("Held draft", "held.example:9000");
  data.client.replace.mockRejectedValueOnce(new Error("Save outcome unknown")); data.client.snapshot.mockRejectedValueOnce(new Error("Read unavailable"));
  await press("Save server"); expect(screen.getByLabelText("Connection name")).toHaveDisplayValue("Held draft"); expect(screen.getByText("Save server")).toBeDisabled(); expect(screen.queryByText("Review current settings")).toBeNull();
  await tick(); expect(data.client.replace).toHaveBeenCalledTimes(1); await press("Check saved settings"); expect(screen.getByText("Review current settings")).toBeEnabled(); expect(data.client.replace).toHaveBeenCalledTimes(1);
});
test("a stale poll cannot overwrite a later successful save", async () => {
  const data = fixture(); await render(<Connections active />); const stale = data.saved; const reading = deferred<host.ConnectionSnapshot>(); data.client.snapshot.mockReturnValueOnce(reading.promise);
  await tick(); await press("Remove Public node"); expect(screen.queryByText("Public node")).toBeNull(); await act(async () => { reading.resolve(stale); }); expect(screen.queryByText("Public node")).toBeNull(); expect(data.client.replace).toHaveBeenCalledTimes(1);
});
test("Stop/Open replaces live observations without replacing the editor or its original guard", async () => {
  const data = fixture(); const view = await render(<Connections owner={data.owner} active />); await press("Edit Public node"); await enter("Retained editor", "kept.example:4242"); const original = data.saved.guard;
  await view.rerender(<Connections active />); expect(screen.getByLabelText("Connection name")).toHaveDisplayValue("Retained editor"); await view.rerender(<Connections owner={{ ...data.owner } as owners.ServiceRuntimeLike} active />);
  await press("Save server"); expect(data.client.replace).toHaveBeenCalledWith(original, expect.arrayContaining([expect.objectContaining({ label: "Retained editor" })])); expect(app.appConnectionProfiles).toHaveBeenCalledTimes(1);
});
test("native applied revision remains visible when host health fails; retry uses the saved guard", async () => {
  const data = fixture(); data.network.snapshot.mockRejectedValue(new Error("Health read unavailable")); await render(<Connections owner={data.owner} active />);
  expect(screen.getByText("Saved settings are active.")).toBeTruthy(); expect(screen.getByText(/Network health is unavailable/)).toBeTruthy(); await press("Retry Public node"); expect(data.binding.retry).toHaveBeenCalledWith(data.saved.guard, data.tcp.id); expect(data.client.replace).not.toHaveBeenCalled();
});
test("removed presets are not recreated by refresh, remount, or host replacement", async () => {
  const data = fixture(); data.saved = { ...data.saved, profiles: [] }; const view = await render(<Connections active />); expect(screen.queryByText("Refresh connections")).toBeNull(); await view.rerender(<Connections active={false} />); await view.rerender(<Connections owner={data.owner} active />);
  expect(screen.getByText("No saved connections.")).toBeTruthy(); expect(data.client.replace).not.toHaveBeenCalled(); expect(host.newConnectionProfileId).not.toHaveBeenCalled();
});
test("initial process lookup failure can recover when the native owner becomes available", async () => {
  const data = fixture(); jest.mocked(app.appConnectionProfiles).mockImplementationOnce(() => { throw new Error("Not started"); }); const view = await render(<Connections active />);
  expect(screen.getByText(/Saved connections are not available yet/)).toBeTruthy(); await view.rerender(<Connections owner={data.owner} active />); expect(screen.getByText("Public node")).toBeTruthy();
});
test("canonical value reconciliation compares bytes, bigints and enum payloads", () => {
  expect(sameValue({ id: id(1), revision: 3n, value: new host.Bitrate.Auto() }, { id: id(1), revision: 3n, value: new host.Bitrate.Auto() })).toBe(true);
  expect(sameValue(id(1), id(2))).toBe(false); expect(sameValue(id(1), {})).toBe(false); expect(sameValue([1], { 0: 1 })).toBe(false); expect(sameValue(3n, 4n)).toBe(false);
  expect(sameValue(new Uint8Array([9, 1, 2, 8]).subarray(1, 3), new Uint8Array([1, 2]).buffer)).toBe(true);
  expect(sameValue(new Uint8Array(), {})).toBe(false);
});


test("local discovery is restored only by an explicit guarded action", async () => {
  const data = fixture(); data.saved = { ...data.saved, profiles: [] };
  await render(<Connections active />);
  expect(data.client.replace).not.toHaveBeenCalled();
  await press("Add local discovery");
  expect(host.newConnectionProfileId).toHaveBeenCalledTimes(1);
  expect(data.client.replace).toHaveBeenCalledWith(expect.objectContaining({ revision: 1n }), [expect.objectContaining({
    id: id(3), enabled: false, config: expect.objectContaining({ tag: "AutoLan" }),
  })]);
  expect(screen.queryByText("Add local discovery")).toBeNull();
});

test("a conflict that removed the edited server can be cancelled after exact readback", async () => {
  const data = fixture(); await render(<Connections active />);
  await press("Edit Public node"); await enter("My draft", "other.example:4242");
  data.client.replace.mockImplementationOnce(async () => {
    data.saved = { ...data.saved, guard: { ...data.saved.guard, revision: 2n }, profiles: [data.lan] };
    throw new Error("Conflict");
  });
  await press("Save server");
  expect(screen.getByText("Review current settings")).toBeDisabled();
  expect(screen.getByText("Cancel server edit")).toBeEnabled();
  await press("Cancel server edit");
  expect(screen.queryByLabelText("Connection name")).toBeNull();
  expect(screen.getByText("Add TCP server")).toBeEnabled();
  expect(data.client.replace).toHaveBeenCalledTimes(1);
});

test("a newly saved ID is not appended twice after another setting changes before readback", async () => {
  const data = fixture(); await render(<Connections active />);
  await press("Add TCP server"); await enter("New server", "new.example:4242");
  data.client.replace.mockImplementationOnce(async (_guard, profiles) => {
    data.saved = { ...data.saved, guard: { ...data.saved.guard, revision: 3n }, profiles: profiles.map((profile) => profile.id === data.lan.id ? { ...profile, enabled: true } : profile) };
    throw new Error("Reply lost");
  });
  await press("Save server");
  expect(screen.getByText("This server is already saved. Cancel this draft to edit the saved connection.")).toBeTruthy();
  expect(screen.getByText("Review current settings")).toBeDisabled();
  await press("Cancel server edit");
  await press("Edit New server");
  expect(host.newConnectionProfileId).toHaveBeenCalledTimes(1);
  expect(data.client.replace).toHaveBeenCalledTimes(1);
});

test("lost no-op reply confirms desired saved state without claiming or replaying a commit", async () => {
  const data = fixture(); await render(<Connections active />);
  await press("Edit Public node");
  data.client.replace.mockRejectedValueOnce(new Error("Reply lost"));
  await press("Save server");
  expect(screen.getByText("These settings are saved. No repeat save was sent.")).toBeTruthy();
  expect(data.client.replace).toHaveBeenCalledTimes(1);
  expect(data.saved.guard.revision).toBe(1n);
});

test("late old-generation retry completion cannot describe the replacement network", async () => {
  const data = fixture(); const done = deferred<void>(); data.binding.retry.mockReturnValueOnce(done.promise);
  const view = await render(<Connections owner={data.owner} active />);
  await press("Retry Public node");
  await view.rerender(<Connections active />);
  await act(async () => { done.resolve(); });
  expect(screen.queryByText("Retry requested. Connection health is shown separately below.")).toBeNull();
  expect(screen.getByText("Saved connections will apply when networking starts.")).toBeTruthy();
});
