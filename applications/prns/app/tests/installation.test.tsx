jest.mock("@react-native-async-storage/async-storage", () => require("@react-native-async-storage/async-storage/jest/async-storage-mock"));
import { Button, Linking, Text, View } from "react-native";
import AsyncStorage from "@react-native-async-storage/async-storage";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react-native";
import { prns_chat_app as nativeApp, prns_service_runtime as native } from "@prns/app-bindings";
import type { prns_service_runtime_uniffi as owners } from "@prns/app-bindings";
import * as platform from "@prns/app-platform";
import { InstallationProvider, useInstallation } from "../src/runtime/InstallationProvider";
import { Identities } from "../src/Identities";
import { drainPresentationForReset, finishPresentationReset } from "../src/runtime/presentation-storage";

// Lifecycle tests observe the provider directly; routed screen journeys have
// their own tests and do not depend on the old one-screen application layout.
function LifecycleProbe() {
  const value = useInstallation();
  const { owner, mode, status, busy, error } = value;
  return <View>
    <Button title="Open directly" onPress={() => { void value.start("open", false); }} />
    {mode === undefined && error !== undefined && <Button title="Retry startup check" onPress={value.retryInitialization} />}
    {error !== undefined && <Text>{error}</Text>}
    {status !== undefined && <Text>{native.ServiceRuntimePhase[status.phase]}</Text>}
    {status?.latestIdentityDiagnostic !== undefined && <Text>Last reported identity issue: {String(status.latestIdentityDiagnostic.error).replace(/^Error: /, "")}</Text>}
    {owner === undefined && (status === undefined || status.cleanupComplete) && mode !== undefined && <Button
      title={mode === "create" ? "Set up this device" : "Open installation"} disabled={busy !== undefined}
      onPress={() => { void value.start(mode, value.bluetooth); }} />}
    {(status !== undefined || busy !== undefined) && <Button title={busy === "start" ? "Cancel startup" : "Stop networking"}
      disabled={busy === "stop"} onPress={() => { void value.stop(); }} />}
    {owner !== undefined && <Identities owner={owner} active onSelect={() => {}} />}
  </View>;
}
function App() { return <InstallationProvider><LifecycleProbe /></InstallationProvider>; }
import { CreationMetadataRejected, loadCreationDraft, prepareCreationDraft, submitCreationDraft } from "../src/identity-creation";

jest.mock("../src/use-foreground", () => ({ useForeground: () => true }));
jest.mock("../src/IdentityPanel", () => ({ IdentityPanel: () => null }));
jest.mock("../src/CatalogActivity", () => ({ CatalogActivity: () => null }));
jest.mock("../src/Network", () => ({ Network: () => null }));
jest.mock("@prns/app-bindings", () => ({
  prns_chat_app: { appOwner: jest.fn(), applicationOptions: () => ({ catalog: { maxGroups: 32 }, archive: { maxRecords: 100000n, maxBytes: 268435456n } }) },
  prns_service_runtime: { ServiceRuntimeNetworkPhase: { Offline: 0, Ready: 2 }, ServiceRuntimePhase: {
    Starting: 0, Ready: 1, Closing: 2, Failed: 3, Unresolved: 4, Closed: 5,
    0: "Starting", 1: "Ready", 2: "Closing", 3: "Failed", 4: "Unresolved", 5: "Closed",
  } },
  prns_identities_core: { IdentityDesiredState: {} },
}));
jest.mock("@prns/app-platform", () => ({
  installationPresent: jest.fn(), bluetoothSelected: jest.fn(), start: jest.fn(), stop: jest.fn(),
}));
jest.mock("../src/identity-creation", () => ({
  loadCreationDraft: jest.fn().mockResolvedValue(undefined),
  prepareCreationDraft: jest.fn(), submitCreationDraft: jest.fn(), hex: jest.fn(),
  CreationDraftConflict: class extends Error {},
  CreationMetadataRejected: class extends Error {
    readonly draft: unknown;
    constructor(draft: unknown) {
      super("Update the identity names before trying again.");
      this.draft = draft;
    }
  },
}));
jest.mock("expo-status-bar", () => ({ StatusBar: () => null }));
jest.mock("react-native-safe-area-context", () => {
  const { View } = require("react-native");
  return { SafeAreaProvider: View, SafeAreaView: View };
});

function deferred() {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function owner(phase: native.ServiceRuntimePhase, cleanupComplete = false) {
  const snapshot = { phase, cleanupComplete };
  const identities = jest.fn(() => ({ identities: jest.fn().mockResolvedValue([]) }));
  const value = { status: () => ({ ...snapshot }), networkStatus: () => ({ phase: 0, generation: 0n }), identities } as unknown as owners.ServiceRuntimeLike;
  return { value, snapshot, identities };
}
const presence = jest.mocked(platform.installationPresent);
const begin = jest.mocked(platform.start);
const stop = jest.mocked(platform.stop);
const published = jest.mocked(nativeApp.appOwner);
async function tick() { await act(async () => { jest.advanceTimersByTime(250); }); }

beforeEach(() => {
  jest.useFakeTimers();
  jest.resetAllMocks();
  jest.mocked(AsyncStorage.getItem).mockResolvedValue(null);
  jest.spyOn(Linking, "addEventListener").mockReturnValue({ remove: jest.fn() } as unknown as ReturnType<typeof Linking.addEventListener>);
  jest.spyOn(Linking, "getInitialURL").mockResolvedValue(null);
  presence.mockResolvedValue(true);
  jest.mocked(platform.bluetoothSelected).mockResolvedValue(false);
  published.mockImplementation(() => { throw new Error("Not published"); });
  begin.mockResolvedValue(undefined);
  stop.mockResolvedValue(undefined);
});
afterEach(async () => { await cleanup(); jest.useRealTimers(); });

test("fresh installation requires explicit creation and no Bluetooth permission", async () => {
  presence.mockResolvedValue(false);
  await render(<App />);
  expect(begin).not.toHaveBeenCalled();
  const ready = owner(native.ServiceRuntimePhase.Ready);
  begin.mockImplementation(async () => { published.mockReturnValue(ready.value); });
  await act(async () => { await fireEvent.press(screen.getByText("Set up this device")); });
  expect(begin).toHaveBeenCalledWith("create", false);
  expect(screen.getByLabelText("Private identity label")).toBeEnabled();
});

test("a failed reset-journal check fences deep-link startup until a successful retry", async () => {
  jest.mocked(AsyncStorage.getItem).mockImplementation(async (key) => {
    if (key === "prns.app.reset-pending.v1") throw new Error("Reset journal unavailable");
    return null;
  });
  await render(<App />);
  expect(screen.getByText("Reset journal unavailable")).toBeTruthy();
  await act(async () => { await fireEvent.press(screen.getByText("Open directly")); });
  expect(begin).not.toHaveBeenCalled();
  jest.mocked(AsyncStorage.getItem).mockResolvedValue(null);
  await act(async () => { await fireEvent.press(screen.getByText("Retry startup check")); });
  expect(begin).toHaveBeenCalledWith("open", false);
});

test("saved Bluetooth selection is reused when reopening after a JavaScript reload", async () => {
  jest.mocked(platform.bluetoothSelected).mockResolvedValue(true);
  const ready = owner(native.ServiceRuntimePhase.Ready);
  begin.mockImplementation(async () => { published.mockReturnValue(ready.value); });
  await render(<App />);
  expect(begin).toHaveBeenCalledWith("open", true);
  await act(async () => { await fireEvent.press(screen.getByText("Stop networking")); });
  expect(stop).toHaveBeenCalledTimes(1);
  expect(screen.getByLabelText("Private identity label")).toBeEnabled();
  expect(screen.queryByText("Open installation")).toBeNull();
});

test("a transient saved-form read failure can be retried without reopening the installation", async () => {
  const ready = owner(native.ServiceRuntimePhase.Ready);
  published.mockReturnValue(ready.value);
  jest.mocked(loadCreationDraft).mockRejectedValueOnce(new Error("Temporary form storage failure"));
  await render(<App />);
  expect(screen.getByText("Temporary form storage failure")).toBeTruthy();
  expect(screen.getByText("Retry loading identities")).toBeEnabled();
  expect(begin).toHaveBeenCalledTimes(1);
  expect(begin).toHaveBeenCalledWith("open", false);
  jest.mocked(loadCreationDraft).mockResolvedValue(undefined);
  await act(async () => { await fireEvent.press(screen.getByText("Retry loading identities")); });
  expect(screen.queryByText("Temporary form storage failure")).toBeNull();
  expect(screen.getByLabelText("Private identity label")).toBeEnabled();
  expect(screen.queryByText("Retry loading identities")).toBeNull();
  expect(screen.queryByText("Refresh identities")).toBeNull();
  expect(begin).toHaveBeenCalledTimes(1);
  expect(stop).not.toHaveBeenCalled();
});

test("partial creation failure offers strict Open instead of repeating Create", async () => {
  presence.mockResolvedValueOnce(false).mockResolvedValue(true);
  begin.mockRejectedValueOnce(new Error("Creation interrupted after publishing state"));
  await render(<App />);
  await act(async () => { await fireEvent.press(screen.getByText("Set up this device")); });
  expect(screen.getByText("Creation interrupted after publishing state")).toBeTruthy();
  expect(screen.getByText("Open installation")).toBeEnabled();
  await act(async () => { await fireEvent.press(screen.getByText("Open installation")); });
  expect(begin).toHaveBeenLastCalledWith("open", false);
});

test("a retained Closing owner displays its latest cleanup diagnostic while Stop is pending", async () => {
  const retained = owner(native.ServiceRuntimePhase.Ready);
  published.mockReturnValue(retained.value);
  stop.mockReturnValue(deferred().promise);
  await render(<App />);
  await act(async () => { await fireEvent.press(screen.getByText("Stop networking")); });
  Object.assign(retained.snapshot, { phase: native.ServiceRuntimePhase.Closing,
    latestIdentityDiagnostic: { error: new Error("Identity drain is still waiting") } });
  await tick();
  expect(screen.getByText("Closing")).toBeTruthy();
  expect(screen.getByText(/Last reported identity issue: Identity drain is still waiting/)).toBeTruthy();
  expect(screen.queryByText("Open installation")).toBeNull();
});

test("Stop supersedes pending startup before publication and ignores its late completion", async () => {
  const starting = deferred();
  const closing = deferred();
  begin.mockReturnValue(starting.promise);
  stop.mockReturnValue(closing.promise);
  await render(<App />);
  expect(begin).toHaveBeenCalledWith("open", false);
  await act(async () => { await fireEvent.press(screen.getByText("Cancel startup")); });
  expect(stop).toHaveBeenCalledTimes(1);
  await act(async () => { closing.resolve(); });
  const stale = owner(native.ServiceRuntimePhase.Ready);
  published.mockReturnValue(stale.value);
  await act(async () => { starting.resolve(); });
  await tick();
  expect(stale.identities).not.toHaveBeenCalled();
  expect(screen.queryByLabelText("Private identity label")).toBeNull();
  expect(screen.getByText("Open installation")).toBeTruthy();
});

test("retained unresolved startup stays observable and cancellable before readiness", async () => {
  begin.mockReturnValue(deferred().promise);
  await render(<App />);
  const retained = owner(native.ServiceRuntimePhase.Unresolved);
  published.mockReturnValue(retained.value);
  await tick();
  expect(screen.getByText("Unresolved")).toBeTruthy();
  await act(async () => { await fireEvent.press(screen.getByText("Cancel startup")); });
  expect(stop).toHaveBeenCalledTimes(1);
});

test("a previously closed wrapper is never mistaken for the replacing Start", async () => {
  const old = owner(native.ServiceRuntimePhase.Closed, true);
  published.mockReturnValue(old.value);
  const starting = deferred();
  begin.mockReturnValue(starting.promise);
  await render(<App />);
  expect(screen.getByText("Cancel startup")).toBeEnabled();
  const replacement = owner(native.ServiceRuntimePhase.Ready);
  published.mockReturnValue(replacement.value);
  await act(async () => { starting.resolve(); });
  expect(old.identities).not.toHaveBeenCalled();
  expect(replacement.identities).toHaveBeenCalled();
  expect(screen.getByLabelText("Private identity label")).toBeEnabled();
});

test("a transport error preserves the ready owner and its local identity access", async () => {
  const ready = owner(native.ServiceRuntimePhase.Ready);
  begin.mockImplementation(async () => {
    published.mockReturnValue(ready.value);
    throw new Error("Optional transport unavailable");
  });
  await render(<App />);
  expect(screen.getByText("Optional transport unavailable")).toBeTruthy();
  expect(screen.getByLabelText("Private identity label")).toBeEnabled();
});

test("failed drain keeps the owner visible and permits another explicit Stop", async () => {
  const retained = owner(native.ServiceRuntimePhase.Ready);
  published.mockReturnValue(retained.value);
  await render(<App />);
  stop.mockImplementation(async () => {
    retained.snapshot.phase = native.ServiceRuntimePhase.Unresolved;
    throw new Error("Cleanup incomplete");
  });
  await act(async () => { await fireEvent.press(screen.getByText("Stop networking")); });
  expect(screen.getByText("Unresolved")).toBeTruthy();
  expect(screen.queryByLabelText("Private identity label")).toBeNull();
  expect(screen.queryByText("Open installation")).toBeNull();
  expect(screen.getByText("Stop networking")).toBeEnabled();
});

const creation = { version: 2 as const, incarnation: "11".repeat(16), epoch: "1", nonce: "22".repeat(32),
  localLabel: "é".repeat(33), publishedName: "Roof" };

test("definite metadata rejection returns the saved names to an editable form", async () => {
  published.mockReturnValue(owner(native.ServiceRuntimePhase.Ready).value);
  jest.mocked(prepareCreationDraft).mockResolvedValue(creation);
  jest.mocked(submitCreationDraft).mockRejectedValue(new CreationMetadataRejected(creation, new Error("InvalidMetadata")));
  await render(<App />);
  await fireEvent.changeText(screen.getByLabelText("Private identity label"), creation.localLabel);
  await fireEvent.changeText(screen.getByLabelText("Shared identity name"), creation.publishedName);
  await act(async () => { await fireEvent.press(screen.getByText("Create identity")); });
  expect(screen.getByLabelText("Private identity label")).toHaveDisplayValue(creation.localLabel);
  expect(screen.getByLabelText("Private identity label")).toBeEnabled();
  expect(screen.queryByText("Resume saved request")).toBeNull();
  expect(screen.getByText("Update the identity names before trying again.")).toBeTruthy();
});

test("uncertain failure retains the immutable input and retries without minting another ID", async () => {
  published.mockReturnValue(owner(native.ServiceRuntimePhase.Ready).value);
  jest.mocked(prepareCreationDraft).mockResolvedValue(creation);
  jest.mocked(submitCreationDraft).mockRejectedValue(new Error("Storage confirmation lost"));
  await render(<App />);
  await fireEvent.changeText(screen.getByLabelText("Private identity label"), creation.localLabel);
  await fireEvent.changeText(screen.getByLabelText("Shared identity name"), creation.publishedName);
  await act(async () => { await fireEvent.press(screen.getByText("Create identity")); });
  expect(screen.queryByLabelText("Private identity label")).toBeNull();
  await act(async () => { await fireEvent.press(screen.getByText("Resume saved request")); });
  expect(prepareCreationDraft).toHaveBeenCalledTimes(1);
  expect(submitCreationDraft).toHaveBeenCalledTimes(2);
  expect(jest.mocked(submitCreationDraft).mock.calls[0]?.[1]).toBe(creation);
  expect(jest.mocked(submitCreationDraft).mock.calls[1]?.[1]).toBe(creation);
});


test("identity preparation completing after reset cannot save or submit the old request", async () => {
  const preparation = deferred();
  const ready = owner(native.ServiceRuntimePhase.Ready);
  jest.mocked(prepareCreationDraft).mockImplementation(async () => { await preparation.promise; return creation; });
  await render(<Identities owner={ready.value} active onSelect={() => {}} />);
  await fireEvent.changeText(screen.getByLabelText("Private identity label"), creation.localLabel);
  await fireEvent.changeText(screen.getByLabelText("Shared identity name"), creation.publishedName);
  await act(async () => { await fireEvent.press(screen.getByText("Create identity")); });
  expect(prepareCreationDraft).toHaveBeenCalledTimes(1);
  await act(async () => { await drainPresentationForReset(); finishPresentationReset(); });
  await act(async () => { preparation.resolve(); });
  expect(submitCreationDraft).not.toHaveBeenCalled();
  expect(screen.queryByText("Resume saved request")).toBeNull();
  expect(screen.getByText(/earlier view can no longer save changes/)).toBeTruthy();
});
