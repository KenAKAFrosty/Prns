import { act, fireEvent, render, screen } from "@testing-library/react-native";
import type { prns_identities_core as core, prns_service_runtime_uniffi as owners } from "@prns/app-bindings";
import { IdentityAnnouncements } from "../src/IdentityAnnouncements";

jest.mock("@prns/app-bindings", () => ({ prns_identities_runtime: {
  IdentityManualAnnounce_Tags: { Queued: "Queued", Throttled: "Throttled", Disabled: "Disabled" },
} }));
const reference = {} as core.IdentityReference;
const settings = jest.fn();
const save = jest.fn();
const announce = jest.fn();
const owner = { announcements: () => ({ settings, setSettings: save, announce }) } as unknown as owners.ServiceRuntimeLike;
beforeEach(() => {
  jest.clearAllMocks();
  settings.mockResolvedValue({ intervalMillis: 3_600_000n });
  save.mockImplementation(async (value) => value);
  announce.mockResolvedValue({ tag: "Queued" });
});

test("manual-only saves canonical offline preferences without issuing an announcement", async () => {
  await render(<IdentityAnnouncements owner={owner} reference={reference} active />);
  expect(screen.getByText("Schedule: Every 60 minutes")).toBeTruthy();
  await act(async () => { await fireEvent.press(screen.getByText("Manual only")); });
  expect(save).toHaveBeenCalledWith({ intervalMillis: undefined });
  expect(announce).not.toHaveBeenCalled();
  expect(screen.getByText("Schedule: Manual only")).toBeTruthy();
});

test("throttled manual requests explain the wait and connection requirement", async () => {
  announce.mockResolvedValue({ tag: "Throttled", inner: { retryAfterMs: 1501n } });
  await render(<IdentityAnnouncements owner={owner} reference={reference} active />);
  await act(async () => { await fireEvent.press(screen.getByText("Announce this identity now")); });
  expect(screen.getByText(/Announcement queued. It can be sent in about 2 seconds when connected/)).toBeTruthy();
});

test("uncertain preference replies reread committed settings", async () => {
  save.mockRejectedValueOnce(new Error("Reply lost"));
  await render(<IdentityAnnouncements owner={owner} reference={reference} active />);
  settings.mockResolvedValue({ intervalMillis: 900_000n });
  await act(async () => { await fireEvent.press(screen.getByText("15 minutes")); });
  expect(screen.getByText(/Reply lost/)).toBeTruthy();
  expect(screen.getByText("Schedule: Every 15 minutes")).toBeTruthy();
});


function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((done) => { resolve = done; }); return { promise, resolve }; }

test("hiding a pending setting preserves the native action and releases controls when it settles", async () => {
  const waiting = deferred<{ intervalMillis: bigint | undefined }>();
  save.mockReturnValueOnce(waiting.promise);
  const view = await render(<IdentityAnnouncements owner={owner} reference={reference} active />);
  await act(async () => { await fireEvent.press(screen.getByText("Manual only")); });
  await view.rerender(<IdentityAnnouncements owner={owner} reference={reference} active={false} />);
  settings.mockResolvedValue({ intervalMillis: undefined });
  await act(async () => { waiting.resolve({ intervalMillis: undefined }); });
  await view.rerender(<IdentityAnnouncements owner={owner} reference={reference} active />);
  expect(screen.getByText("Schedule: Manual only")).toBeTruthy();
  expect(screen.getByText("Announce this identity now")).toBeEnabled();
  expect(save).toHaveBeenCalledTimes(1);
});

test("returning before a pending setting settles stays busy then publishes only the accepted result", async () => {
  const waiting = deferred<{ intervalMillis: bigint | undefined }>();
  save.mockReturnValueOnce(waiting.promise);
  const view = await render(<IdentityAnnouncements owner={owner} reference={reference} active />);
  await act(async () => { await fireEvent.press(screen.getByText("15 minutes")); });
  await view.rerender(<IdentityAnnouncements owner={owner} reference={reference} active={false} />);
  await view.rerender(<IdentityAnnouncements owner={owner} reference={reference} active />);
  expect(screen.getByText("Announce this identity now")).toBeDisabled();
  await act(async () => { waiting.resolve({ intervalMillis: 900_000n }); });
  expect(screen.getByText("Schedule: Every 15 minutes")).toBeTruthy();
  expect(screen.getByText("Announce this identity now")).toBeEnabled();
});

test("an old client's late setting and announcement replies cannot overwrite a replacement client", async () => {
  const waiting = deferred<{ intervalMillis: bigint | undefined }>();
  save.mockReturnValueOnce(waiting.promise);
  const view = await render(<IdentityAnnouncements owner={owner} reference={reference} active />);
  await act(async () => { await fireEvent.press(screen.getByText("Manual only")); });
  const nextAnnounce = deferred<{ tag: string }>();
  const next = { announcements: () => ({ settings: async () => ({ intervalMillis: 1_800_000n }), setSettings: async (value: unknown) => value,
    announce: () => nextAnnounce.promise }) } as unknown as owners.ServiceRuntimeLike;
  await view.rerender(<IdentityAnnouncements owner={next} reference={reference} active />);
  await act(async () => { waiting.resolve({ intervalMillis: undefined }); });
  expect(screen.getByText("Schedule: Every 30 minutes")).toBeTruthy();
  expect(screen.getByText("Announce this identity now")).toBeEnabled();
  await act(async () => { await fireEvent.press(screen.getByText("Announce this identity now")); });
  await view.rerender(<IdentityAnnouncements owner={owner} reference={reference} active />);
  await act(async () => { nextAnnounce.resolve({ tag: "Queued" }); });
  expect(screen.queryByText("Announcement queued.")).toBeNull();
  expect(screen.getByText("Schedule: Every 60 minutes")).toBeTruthy();
});


test("announcement read recovery is offered only after failure; manual announce remains explicit", async () => {
  settings.mockRejectedValueOnce(new Error("Settings unavailable"));
  await render(<IdentityAnnouncements owner={owner} reference={reference} active />);
  expect(screen.getByText("Error: Settings unavailable")).toBeTruthy();
  await fireEvent.press(screen.getByRole("button", { name: "Retry loading announcement settings" }));
  await screen.findByText("Schedule: Every 60 minutes");
  expect(screen.queryByText("Refresh announcement preferences")).toBeNull();
  expect(screen.queryByText("Retry loading announcement settings")).toBeNull();
  expect(announce).not.toHaveBeenCalled(); expect(save).not.toHaveBeenCalled();
});


test("compact discovery controls expose one explicit announce action and fold schedule choices", async () => {
  const binding = jest.spyOn(owner, "announcements");
  try {
    await render(<IdentityAnnouncements owner={owner} reference={reference} active compact />);
    expect(screen.getByRole("button", { name: "Announce yourself" })).toBeEnabled();
    expect(screen.queryByText("Manual only")).toBeNull();
    expect(announce).not.toHaveBeenCalled();
    await act(async () => { await fireEvent.press(screen.getByText("Announcement schedule")); });
    expect(screen.getByText("Schedule: Every 60 minutes")).toBeTruthy();
    await act(async () => { await fireEvent.press(screen.getByText("Manual only")); });
    expect(save).toHaveBeenCalledWith({ intervalMillis: undefined });
    expect(announce).not.toHaveBeenCalled();
    await act(async () => { await fireEvent.press(screen.getByText("Announcement schedule")); });
    expect(screen.queryByText("Manual only")).toBeNull();
    await act(async () => { await fireEvent.press(screen.getByText("Announce yourself")); });
    expect(announce).toHaveBeenCalledTimes(1);
    expect(screen.getByText("Announcement queued.")).toBeTruthy();
    expect(binding).toHaveBeenCalledTimes(1);
  } finally { binding.mockRestore(); }
});
