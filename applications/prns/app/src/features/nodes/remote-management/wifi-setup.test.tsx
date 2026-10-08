import * as Bindings from "@prns-internal/expo";
import { act, fireEvent, render, waitFor } from "@testing-library/react-native";
import type { EffectCallback } from "react";
import { AppState, type AppStateStatus } from "react-native";

import type {
  DevelopmentRuntimeView,
  RuntimeCommandResult,
} from "@/native/development-runtime-context";
import { WifiSetupCard, supportsWifiSetup, utf8Length, wifiTransactionMessage } from "./wifi-setup";

let mockFocused = true;
jest.mock("expo-router", () => ({
  useFocusEffect: (effect: EffectCallback) => {
    jest
      .requireActual<typeof import("react")>("react")
      .useEffect(() => (mockFocused ? effect() : undefined), [effect, mockFocused]);
  },
}));

const target = new Uint8Array(16).fill(1);
type Outcome = RuntimeCommandResult<Bindings.RemoteWifiCommandOutcome>;
let nextId = 0n;
function observed(
  transaction: Bindings.RemoteWifiTransaction,
  action = Bindings.RemoteWifiAction.Inspect,
): Outcome {
  return accepted(Bindings.RemoteWifiStatus.Observed.new({ transaction }), action);
}
function accepted(
  status: Bindings.RemoteWifiStatus,
  action = Bindings.RemoteWifiAction.Inspect,
): Outcome {
  return {
    type: "outcome",
    outcome: Bindings.RemoteWifiCommandOutcome.Accepted.new({
      operation: {
        operationId: ++nextId,
        generationId: 1n,
        targetIdentityFingerprint: target,
        action,
        candidateRevision: undefined,
        status,
      },
    }),
  };
}
function fixture(
  transaction: Bindings.RemoteWifiTransaction = Bindings.RemoteWifiTransaction.FactoryProvisioning.new(),
) {
  const inspectRemoteWifiTrial = jest
    .fn<Promise<Outcome>, [Bindings.InspectRemoteWifiTrialInput]>()
    .mockImplementation(async () => observed(transaction));
  const startRemoteWifiTrial = jest
    .fn<Promise<Outcome>, [Bindings.StartRemoteWifiTrialInput]>()
    .mockImplementation(async () =>
      observed(
        Bindings.RemoteWifiTransaction.AwaitingConfirmation.new({
          revision: 17,
          remainingSeconds: 100,
        }),
        Bindings.RemoteWifiAction.Start,
      ),
    );
  const finishRemoteWifiTrial = jest
    .fn<Promise<Outcome>, [Bindings.FinishRemoteWifiTrialInput]>()
    .mockImplementation(async () =>
      observed(
        Bindings.RemoteWifiTransaction.Confirmed.new({ revision: 17 }),
        Bindings.RemoteWifiAction.Keep,
      ),
    );
  const refreshSnapshot = jest
    .fn()
    .mockResolvedValue({ type: "operationFailure", detail: "Unavailable" });
  const snapshot = {
    generationId: 1n,
    runtime: Bindings.DevelopmentNodeRuntime.Running,
    activeOperation: undefined,
    lastRemoteWifi: undefined,
  } as Bindings.DevelopmentNodeSnapshot;
  const runtime = {
    snapshot,
    inspectRemoteWifiTrial,
    startRemoteWifiTrial,
    finishRemoteWifiTrial,
    refreshSnapshot,
  } as unknown as DevelopmentRuntimeView;
  return {
    runtime,
    snapshot,
    inspectRemoteWifiTrial,
    startRemoteWifiTrial,
    finishRemoteWifiTrial,
    refreshSnapshot,
  };
}

beforeEach(() => {
  mockFocused = true;
  nextId = 0n;
});
afterEach(() => jest.restoreAllMocks());

test("clears drafts on background and rechecks on foreground without replaying a write", async () => {
  let changeState: ((state: AppStateStatus) => void) | undefined;
  jest.spyOn(AppState, "addEventListener").mockImplementation((_event, listener) => {
    changeState = listener;
    return { remove: jest.fn() };
  });
  const { runtime, inspectRemoteWifiTrial, startRemoteWifiTrial } = fixture();
  const view = render(<WifiSetupCard target={target} runtime={runtime} busy={false} />);
  await view.findByLabelText("Network name");
  fireEvent.changeText(view.getByLabelText("Network name"), "Camp");
  fireEvent.changeText(view.getByLabelText("Network password"), "secret");
  act(() => changeState?.("background"));
  expect(view.queryByLabelText("Network password")).toBeNull();
  expect(inspectRemoteWifiTrial).toHaveBeenCalledTimes(1);
  act(() => changeState?.("active"));
  await view.findByLabelText("Network password");
  expect(view.getByLabelText("Network password").props.value).toBe("");
  expect(inspectRemoteWifiTrial).toHaveBeenCalledTimes(2);
  expect(startRemoteWifiTrial).not.toHaveBeenCalled();
});

test("requires all transactional Wi-Fi capabilities and counts UTF-8 bytes", () => {
  const kinds = [
    Bindings.RemoteControlRequestKind.StageWifiCredentials,
    Bindings.RemoteControlRequestKind.ActivateWifiCredentials,
    Bindings.RemoteControlRequestKind.InspectWifiTransaction,
    Bindings.RemoteControlRequestKind.ConfirmWifiCredentials,
    Bindings.RemoteControlRequestKind.CancelWifiCredentials,
  ];
  expect(supportsWifiSetup(kinds)).toBe(true);
  for (const kind of kinds)
    expect(supportsWifiSetup(kinds.filter((candidate) => candidate !== kind))).toBe(false);
  expect(utf8Length("camp")).toBe(4);
  expect(utf8Length("é山🌲")).toBe(9);
});

test("inspects once on focus, then waits for an explicit request rather than polling", async () => {
  const { runtime, inspectRemoteWifiTrial } = fixture();
  const view = render(<WifiSetupCard target={target} runtime={runtime} busy={false} />);
  await view.findByLabelText("Network name");
  expect(inspectRemoteWifiTrial).toHaveBeenCalledTimes(1);
  view.rerender(<WifiSetupCard target={target} runtime={{ ...runtime }} busy={false} />);
  expect(inspectRemoteWifiTrial).toHaveBeenCalledTimes(1);
  fireEvent.press(view.getByRole("button", { name: "Check network status" }));
  await waitFor(() => expect(inspectRemoteWifiTrial).toHaveBeenCalledTimes(2));
});

test("defers inspection while another operation is active", async () => {
  const { runtime, inspectRemoteWifiTrial } = fixture();
  const view = render(<WifiSetupCard target={target} runtime={runtime} busy />);
  expect(inspectRemoteWifiTrial).not.toHaveBeenCalled();
  expect(view.queryByLabelText("Network name")).toBeNull();
  view.rerender(<WifiSetupCard target={target} runtime={runtime} busy={false} />);
  await view.findByLabelText("Network name");
  expect(inspectRemoteWifiTrial).toHaveBeenCalledTimes(1);
});

test("checks byte limits, confirms the trial and clears credentials after submission", async () => {
  const { runtime, startRemoteWifiTrial, inspectRemoteWifiTrial } = fixture();
  const view = render(<WifiSetupCard target={target} runtime={runtime} busy={false} />);
  await view.findByLabelText("Network name");
  fireEvent.changeText(view.getByLabelText("Network name"), "山".repeat(11));
  expect(view.getByRole("button", { name: "Try network" })).toBeDisabled();
  fireEvent.changeText(view.getByLabelText("Network name"), "Camp");
  fireEvent.changeText(view.getByLabelText("Network password"), "é".repeat(33));
  expect(view.getByRole("button", { name: "Try network" })).toBeDisabled();
  fireEvent.changeText(view.getByLabelText("Network password"), "private network password");
  expect(view.getByLabelText("Network password").props.secureTextEntry).toBe(true);
  fireEvent.press(view.getByRole("button", { name: "Try network" }));
  expect(startRemoteWifiTrial).not.toHaveBeenCalled();
  fireEvent.press(view.getByRole("button", { name: "Start network trial" }));
  await view.findByRole("button", { name: "Keep this network" });
  expect(startRemoteWifiTrial).toHaveBeenCalledWith({
    targetIdentityFingerprint: target,
    ssid: "Camp",
    password: "private network password",
  });
  expect(view.queryByLabelText("Network password")).toBeNull();
  // A new observation can reopen the form, but must not restore its secrets.
  inspectRemoteWifiTrial.mockResolvedValueOnce(
    observed(Bindings.RemoteWifiTransaction.Confirmed.new({ revision: 17 })),
  );
  fireEvent.press(view.getByRole("button", { name: "Check network status" }));
  await view.findByLabelText("Network password");
  expect(view.getByLabelText("Network password").props.value).toBe("");
  expect(view.getByLabelText("Network name").props.value).toBe("");
});

test("only keeps the exact observed revision after warning that Bluetooth is not Wi-Fi proof", async () => {
  const { runtime, finishRemoteWifiTrial } = fixture(
    Bindings.RemoteWifiTransaction.AwaitingConfirmation.new({ revision: 91, remainingSeconds: 40 }),
  );
  const view = render(<WifiSetupCard target={target} runtime={runtime} busy={false} />);
  fireEvent.press(await view.findByRole("button", { name: "Keep this network" }));
  expect(view.getByText(/working Bluetooth connection does not confirm/)).toBeTruthy();
  expect(finishRemoteWifiTrial).not.toHaveBeenCalled();
  fireEvent.press(view.getByRole("button", { name: "Save network" }));
  await waitFor(() =>
    expect(finishRemoteWifiTrial).toHaveBeenCalledWith({
      targetIdentityFingerprint: target,
      revision: 91,
      decision: Bindings.RemoteWifiDecision.Keep,
    }),
  );
});

test("a zero remaining observation neither declares expiry nor permits keeping the trial", async () => {
  const { runtime, finishRemoteWifiTrial } = fixture(
    Bindings.RemoteWifiTransaction.AwaitingConfirmation.new({ revision: 91, remainingSeconds: 0 }),
  );
  const view = render(<WifiSetupCard target={target} runtime={runtime} busy={false} />);
  expect(await view.findByRole("button", { name: "Keep this network" })).toBeDisabled();
  expect(
    view.getByText("Check network status to see whether this trial can still be saved."),
  ).toBeTruthy();
  expect(view.queryByText(/0 seconds remained/)).toBeNull();
  expect(view.getByRole("button", { name: "Restore previous network" })).not.toBeDisabled();
  expect(finishRemoteWifiTrial).not.toHaveBeenCalled();
});

test("a staged transaction can only be restored, never resumed or kept", async () => {
  const { runtime, finishRemoteWifiTrial } = fixture(
    Bindings.RemoteWifiTransaction.Staged.new({ revision: 12 }),
  );
  const view = render(<WifiSetupCard target={target} runtime={runtime} busy={false} />);
  fireEvent.press(await view.findByRole("button", { name: "Restore previous network" }));
  expect(view.queryByRole("button", { name: "Keep this network" })).toBeNull();
  expect(view.queryByLabelText("Network name")).toBeNull();
  fireEvent.press(view.getByRole("button", { name: "Restore network" }));
  await waitFor(() =>
    expect(finishRemoteWifiTrial).toHaveBeenCalledWith({
      targetIdentityFingerprint: target,
      revision: 12,
      decision: Bindings.RemoteWifiDecision.Restore,
    }),
  );
});

test("rolling back and expired confirmation never imply successful restoration", async () => {
  const { runtime, inspectRemoteWifiTrial } = fixture(
    Bindings.RemoteWifiTransaction.RollingBack.new({ rejectedRevision: 7 }),
  );
  const view = render(<WifiSetupCard target={target} runtime={runtime} busy={false} />);
  await view.findByText(/restoring its previous network/);
  expect(view.queryByLabelText("Network name")).toBeNull();
  expect(view.queryByRole("button", { name: "Keep this network" })).toBeNull();
  inspectRemoteWifiTrial.mockResolvedValueOnce(
    observed(
      Bindings.RemoteWifiTransaction.AwaitingConfirmation.new({ revision: 7, remainingSeconds: 0 }),
    ),
  );
  fireEvent.press(view.getByRole("button", { name: "Check network status" }));
  expect(await view.findByRole("button", { name: "Keep this network" })).toBeDisabled();
  expect(view.queryByLabelText("Network name")).toBeNull();
  expect(
    wifiTransactionMessage(Bindings.RemoteWifiTransaction.RollingBack.new({ rejectedRevision: 7 })),
  ).not.toMatch(/has restored|saved/u);
});

test("a lost admission reply never repeats a write and can recover by inspection", async () => {
  const { runtime, startRemoteWifiTrial, inspectRemoteWifiTrial } = fixture();
  startRemoteWifiTrial.mockResolvedValueOnce({ type: "operationFailure", detail: "Lost reply" });
  const view = render(<WifiSetupCard target={target} runtime={runtime} busy={false} />);
  await view.findByLabelText("Network name");
  fireEvent.changeText(view.getByLabelText("Network name"), "Camp");
  fireEvent.changeText(view.getByLabelText("Network password"), "secret");
  fireEvent.press(view.getByRole("button", { name: "Try network" }));
  fireEvent.press(view.getByRole("button", { name: "Start network trial" }));
  await view.findByText(/No change has been repeated/);
  expect(view.getByRole("button", { name: "Check network status" })).toBeEnabled();
  mockFocused = false;
  view.rerender(<WifiSetupCard target={target} runtime={runtime} busy={false} />);
  mockFocused = true;
  view.rerender(<WifiSetupCard target={target} runtime={runtime} busy={false} />);
  await view.findByLabelText("Network password");
  expect(startRemoteWifiTrial).toHaveBeenCalledTimes(1);
  expect(inspectRemoteWifiTrial).toHaveBeenCalledTimes(2);
  expect(view.getByLabelText("Network password").props.value).toBe("");
});

test("a failed Keep needs fresh inspection before showing another decision", async () => {
  const { runtime, finishRemoteWifiTrial, inspectRemoteWifiTrial } = fixture(
    Bindings.RemoteWifiTransaction.AwaitingConfirmation.new({ revision: 14, remainingSeconds: 50 }),
  );
  finishRemoteWifiTrial.mockResolvedValueOnce(
    accepted(
      Bindings.RemoteWifiStatus.Failed.new({
        stage: Bindings.RemoteManagementFailureStage.Busy,
        detail: "not connected",
      }),
      Bindings.RemoteWifiAction.Keep,
    ),
  );
  const view = render(<WifiSetupCard target={target} runtime={runtime} busy={false} />);
  fireEvent.press(await view.findByRole("button", { name: "Keep this network" }));
  fireEvent.press(view.getByRole("button", { name: "Save network" }));
  await view.findByText(/not ready to keep this network/);
  expect(view.queryByRole("button", { name: "Keep this network" })).toBeNull();
  fireEvent.press(view.getByRole("button", { name: "Check network status" }));
  await view.findByRole("button", { name: "Keep this network" });
  expect(finishRemoteWifiTrial).toHaveBeenCalledTimes(1);
  expect(inspectRemoteWifiTrial).toHaveBeenCalledTimes(2);
});

test("late admission replies from an earlier focus cannot enable stale decisions", async () => {
  const { runtime, inspectRemoteWifiTrial } = fixture();
  let finishOld: ((outcome: Outcome) => void) | undefined;
  inspectRemoteWifiTrial.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finishOld = resolve;
      }),
  );
  const view = render(<WifiSetupCard target={target} runtime={runtime} busy={false} />);
  mockFocused = false;
  view.rerender(<WifiSetupCard target={target} runtime={runtime} busy={false} />);
  mockFocused = true;
  view.rerender(<WifiSetupCard target={target} runtime={runtime} busy={false} />);
  await act(async () => {
    finishOld?.(
      observed(
        Bindings.RemoteWifiTransaction.AwaitingConfirmation.new({
          revision: 2,
          remainingSeconds: 50,
        }),
      ),
    );
  });
  await view.findByLabelText("Network name");
  expect(view.queryByRole("button", { name: "Keep this network" })).toBeNull();
  expect(inspectRemoteWifiTrial).toHaveBeenCalledTimes(2);
});

test("pending operations stay disabled until the actor publishes its terminal observation", async () => {
  const { runtime, snapshot, inspectRemoteWifiTrial } = fixture();
  const pending = accepted(Bindings.RemoteWifiStatus.Pending.new());
  inspectRemoteWifiTrial.mockResolvedValueOnce(pending);
  if (
    pending.type !== "outcome" ||
    pending.outcome.tag !== Bindings.RemoteWifiCommandOutcome_Tags.Accepted
  )
    throw new Error("fixture");
  const operation = pending.outcome.inner.operation;
  const view = render(<WifiSetupCard target={target} runtime={runtime} busy={false} />);
  await waitFor(() => expect(inspectRemoteWifiTrial).toHaveBeenCalledTimes(1));
  expect(view.getByRole("button", { name: "Check network status" })).toBeDisabled();
  expect(view.queryByLabelText("Network name")).toBeNull();
  view.rerender(
    <WifiSetupCard
      target={target}
      runtime={{
        ...runtime,
        snapshot: {
          ...snapshot,
          lastRemoteWifi: {
            ...operation,
            status: Bindings.RemoteWifiStatus.Observed.new({
              transaction: Bindings.RemoteWifiTransaction.Confirmed.new({ revision: 1 }),
            }),
          },
        },
      }}
      busy={false}
    />,
  );
  await view.findByLabelText("Network name");
  expect(inspectRemoteWifiTrial).toHaveBeenCalledTimes(1);
});

test("a generation change requires new inspection and rejects an old generation's reply", async () => {
  const { runtime, snapshot, inspectRemoteWifiTrial } = fixture();
  let finishOld: ((outcome: Outcome) => void) | undefined;
  inspectRemoteWifiTrial.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finishOld = resolve;
      }),
  );
  const newResult = observed(Bindings.RemoteWifiTransaction.FactoryProvisioning.new());
  if (
    newResult.type !== "outcome" ||
    newResult.outcome.tag !== Bindings.RemoteWifiCommandOutcome_Tags.Accepted
  )
    throw new Error("fixture");
  inspectRemoteWifiTrial.mockResolvedValueOnce({
    type: "outcome",
    outcome: Bindings.RemoteWifiCommandOutcome.Accepted.new({
      operation: { ...newResult.outcome.inner.operation, generationId: 2n },
    }),
  });
  const view = render(<WifiSetupCard target={target} runtime={runtime} busy={false} />);
  view.rerender(
    <WifiSetupCard
      target={target}
      runtime={{ ...runtime, snapshot: { ...snapshot, generationId: 2n } }}
      busy={false}
    />,
  );
  await act(async () => {
    finishOld?.(
      observed(
        Bindings.RemoteWifiTransaction.AwaitingConfirmation.new({
          revision: 2,
          remainingSeconds: 50,
        }),
      ),
    );
  });
  await view.findByLabelText("Network name");
  expect(view.queryByRole("button", { name: "Keep this network" })).toBeNull();
  expect(inspectRemoteWifiTrial).toHaveBeenCalledTimes(2);
});

test("another target replacing the last-operation slot does not leave an old pending inspection stuck", async () => {
  const { runtime, snapshot, inspectRemoteWifiTrial } = fixture();
  const pending = accepted(Bindings.RemoteWifiStatus.Pending.new());
  if (
    pending.type !== "outcome" ||
    pending.outcome.tag !== Bindings.RemoteWifiCommandOutcome_Tags.Accepted
  )
    throw new Error("fixture");
  inspectRemoteWifiTrial.mockResolvedValueOnce(pending);
  const view = render(<WifiSetupCard target={target} runtime={runtime} busy={false} />);
  await waitFor(() => expect(inspectRemoteWifiTrial).toHaveBeenCalledTimes(1));
  view.rerender(
    <WifiSetupCard
      target={target}
      runtime={{
        ...runtime,
        snapshot: {
          ...snapshot,
          lastRemoteWifi: {
            ...pending.outcome.inner.operation,
            operationId: 20n,
            targetIdentityFingerprint: new Uint8Array(16).fill(8),
            status: Bindings.RemoteWifiStatus.Observed.new({
              transaction: Bindings.RemoteWifiTransaction.FactoryProvisioning.new(),
            }),
          },
        },
      }}
      busy={false}
    />,
  );
  await view.findByText(/No change has been repeated/);
  expect(view.getByRole("button", { name: "Check network status" })).toBeEnabled();
});
