import * as Bindings from "@prns-internal/expo";
import { act, fireEvent, render, waitFor } from "@testing-library/react-native";
import type { EffectCallback } from "react";

import type {
  DevelopmentRuntimeView,
  RuntimeCommandResult,
} from "@/native/development-runtime-context";
import { RemoteManagementPanel } from "../remote-management-panel";

let mockFocused = true;
jest.mock("expo-router", () => ({
  useFocusEffect: (effect: EffectCallback) => {
    jest
      .requireActual<typeof import("react")>("react")
      .useEffect(() => (mockFocused ? effect() : undefined), [effect, mockFocused]);
  },
}));

const target = new Uint8Array(16).fill(1);
type ReadResult = RuntimeCommandResult<Bindings.ReadRemoteNodeOutcome>;
const overview = (firmware = "Current firmware"): ReadResult => ({
  type: "outcome",
  outcome: Bindings.ReadRemoteNodeOutcome.Read.new({
    availableRequests: [
      Bindings.RemoteControlRequestKind.Describe,
      Bindings.RemoteControlRequestKind.SetDisplayVisibility,
    ],
    rttMillis: 1n,
    data: Bindings.RemoteNodeData.Overview.new({
      overview: { firmware, power: undefined, interfaces: undefined },
    }),
  }),
});

function fixture() {
  const readRemoteNode = jest
    .fn<Promise<ReadResult>, [Bindings.ReadRemoteNodeInput, AbortSignal?]>()
    .mockResolvedValue(overview());
  const changeRemoteNode = jest.fn<
    Promise<RuntimeCommandResult<Bindings.ChangeRemoteNodeOutcome>>,
    [Bindings.ChangeRemoteNodeInput]
  >();
  const refreshSnapshot = jest
    .fn()
    .mockResolvedValue({ type: "operationFailure", detail: "Unavailable" });
  const snapshot = {
    generationId: 1n,
    runtime: Bindings.DevelopmentNodeRuntime.Running,
    activeOperation: undefined,
    lastRemoteChange: undefined,
  } as Bindings.DevelopmentNodeSnapshot;
  const runtime = {
    snapshot,
    readRemoteNode,
    changeRemoteNode,
    refreshSnapshot,
  } as unknown as DevelopmentRuntimeView;
  return { runtime, snapshot, readRemoteNode, changeRemoteNode, refreshSnapshot };
}

beforeEach(() => {
  mockFocused = true;
});

test("loads once on focus, separates settings sections, and does not poll on rerenders", async () => {
  const { runtime, readRemoteNode } = fixture();
  const view = render(<RemoteManagementPanel target={target} runtime={runtime} />);
  expect(view.getByText("Loading node settings…")).toBeTruthy();
  expect(view.getByRole("button", { name: "Loading…" })).toBeDisabled();
  fireEvent.press(await view.findByRole("tab", { name: "Information" }));
  expect(view.getByText("Current firmware")).toBeTruthy();
  expect(view.queryByRole("button", { name: "Show display" })).toBeNull();
  fireEvent.press(view.getByRole("tab", { name: "Device" }));
  expect(view.getByRole("button", { name: "Show display" })).toBeEnabled();
  view.rerender(<RemoteManagementPanel target={target} runtime={{ ...runtime }} />);
  expect(readRemoteNode).toHaveBeenCalledTimes(1);
  expect(readRemoteNode.mock.calls[0]?.[0].query.tag).toBe(Bindings.RemoteNodeQuery_Tags.Overview);
});

test("cancels on blur and ignores a late reply after a new focus read", async () => {
  const { runtime, readRemoteNode } = fixture();
  let finishOld: ((result: ReadResult) => void) | undefined;
  readRemoteNode.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finishOld = resolve;
      }),
  );
  const view = render(<RemoteManagementPanel target={target} runtime={runtime} />);
  const signal = readRemoteNode.mock.calls[0]?.[1];
  mockFocused = false;
  view.rerender(<RemoteManagementPanel target={target} runtime={runtime} />);
  expect(signal?.aborted).toBe(true);
  mockFocused = true;
  view.rerender(<RemoteManagementPanel target={target} runtime={runtime} />);
  fireEvent.press(await view.findByRole("tab", { name: "Information" }));
  await act(async () => {
    finishOld?.(overview("Obsolete firmware"));
  });
  expect(view.getByText("Current firmware")).toBeTruthy();
  expect(view.queryByText("Obsolete firmware")).toBeNull();
  expect(readRemoteNode).toHaveBeenCalledTimes(2);
});

test("defers the focus read while another operation owns the connection", async () => {
  const { runtime, snapshot, readRemoteNode } = fixture();
  const occupied = {
    ...runtime,
    snapshot: {
      ...snapshot,
      activeOperation: {
        kind: Bindings.DevelopmentNodeOperationKind.Describe,
        startedAtMillis: 0n,
      },
    },
  };
  const view = render(<RemoteManagementPanel target={target} runtime={occupied} />);
  expect(readRemoteNode).not.toHaveBeenCalled();
  expect(view.getByText("Waiting for another operation to finish…")).toBeTruthy();
  view.rerender(<RemoteManagementPanel target={target} runtime={runtime} />);
  await view.findByRole("tab", { name: "Device" });
  expect(readRemoteNode).toHaveBeenCalledTimes(1);
});

test("a failed automatic read needs a deliberate retry, not snapshot polling", async () => {
  const { runtime, readRemoteNode } = fixture();
  readRemoteNode.mockResolvedValueOnce({ type: "operationFailure", detail: "Link lost" });
  const view = render(<RemoteManagementPanel target={target} runtime={runtime} />);
  await view.findByRole("button", { name: "Try again" });
  view.rerender(<RemoteManagementPanel target={target} runtime={{ ...runtime }} />);
  expect(readRemoteNode).toHaveBeenCalledTimes(1);
  fireEvent.press(view.getByRole("button", { name: "Try again" }));
  await view.findByRole("tab", { name: "Device" });
  expect(readRemoteNode).toHaveBeenCalledTimes(2);
});

test("a runtime generation change cancels old evidence and starts a fresh read", async () => {
  const { runtime, snapshot, readRemoteNode } = fixture();
  let finishOld: ((result: ReadResult) => void) | undefined;
  readRemoteNode.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finishOld = resolve;
      }),
  );
  const view = render(<RemoteManagementPanel target={target} runtime={runtime} />);
  const signal = readRemoteNode.mock.calls[0]?.[1];
  view.rerender(
    <RemoteManagementPanel
      target={target}
      runtime={{ ...runtime, snapshot: { ...snapshot, generationId: 2n } }}
    />,
  );
  expect(signal?.aborted).toBe(true);
  fireEvent.press(await view.findByRole("tab", { name: "Information" }));
  await act(async () => {
    finishOld?.(overview("Old generation"));
  });
  expect(view.queryByText("Old generation")).toBeNull();
  expect(readRemoteNode).toHaveBeenCalledTimes(2);
});

test("a lost write admission reply is never retried, including after refocus", async () => {
  const { runtime, readRemoteNode, changeRemoteNode, refreshSnapshot } = fixture();
  changeRemoteNode.mockResolvedValue({ type: "operationFailure", detail: "Reply lost" });
  const view = render(<RemoteManagementPanel target={target} runtime={runtime} />);
  fireEvent.press(await view.findByRole("tab", { name: "Device" }));
  fireEvent.press(view.getByRole("button", { name: "Show display" }));
  await view.findByText(/could not confirm whether the change was accepted/);
  expect(refreshSnapshot).toHaveBeenCalledTimes(1);
  expect(view.queryByRole("button", { name: "Show display" })).toBeNull();
  expect(view.getByRole("button", { name: "Refresh node information" })).toBeDisabled();
  mockFocused = false;
  view.rerender(<RemoteManagementPanel target={target} runtime={runtime} />);
  mockFocused = true;
  view.rerender(<RemoteManagementPanel target={target} runtime={runtime} />);
  await waitFor(() => expect(changeRemoteNode).toHaveBeenCalledTimes(1));
  expect(readRemoteNode).toHaveBeenCalledTimes(1);
});

test("accepted changes leave prior information stale until an explicit fresh read", async () => {
  const { runtime, readRemoteNode, changeRemoteNode } = fixture();
  changeRemoteNode.mockResolvedValue({
    type: "outcome",
    outcome: Bindings.ChangeRemoteNodeOutcome.Accepted.new({
      operation: {
        operationId: 1n,
        generationId: 1n,
        targetIdentityFingerprint: target,
        change: Bindings.RemoteNodeChange.DisplayVisibility.new({ visible: true }),
        status: Bindings.RemoteChangeStatus.Applied.new(),
      },
    }),
  });
  const view = render(<RemoteManagementPanel target={target} runtime={runtime} />);
  fireEvent.press(await view.findByRole("tab", { name: "Device" }));
  fireEvent.press(view.getByRole("button", { name: "Show display" }));
  await view.findByText("Showing the last information received. Refresh before changing settings.");
  expect(readRemoteNode).toHaveBeenCalledTimes(1);
  expect(view.queryByRole("button", { name: "Show display" })).toBeNull();
  fireEvent.press(view.getByRole("button", { name: "Refresh node information" }));
  await view.findByRole("button", { name: "Show display" });
  expect(readRemoteNode).toHaveBeenCalledTimes(2);
  expect(changeRemoteNode).toHaveBeenCalledTimes(1);
});

test("access inventory pages preserve identities and revoke once with refresh guidance", async () => {
  const { runtime, snapshot, readRemoteNode, changeRemoteNode } = fixture();
  const self = new Uint8Array(16).fill(3);
  const other = new Uint8Array(16).fill(4);
  const last = new Uint8Array(16).fill(5);
  const permissions = [
    Bindings.RemoteControlRequestKind.Describe,
    Bindings.RemoteControlRequestKind.InventoryControllers,
    Bindings.RemoteControlRequestKind.RevokeController,
  ];
  const accessRuntime = {
    ...runtime,
    snapshot: {
      ...snapshot,
      pairedTargets: [
        {
          targetIdentityFingerprint: target,
          controllerIdentityFingerprint: self,
          destination: new Uint8Array(16),
          permittedRequests: permissions,
        },
      ],
    },
  };
  readRemoteNode.mockResolvedValueOnce({
    type: "outcome",
    outcome: Bindings.ReadRemoteNodeOutcome.Read.new({
      availableRequests: permissions,
      rttMillis: 1n,
      data: Bindings.RemoteNodeData.Overview.new({
        overview: { firmware: undefined, power: undefined, interfaces: undefined },
      }),
    }),
  });
  const controllerPage = (identities: Uint8Array[], next?: Uint8Array): ReadResult => ({
    type: "outcome",
    outcome: Bindings.ReadRemoteNodeOutcome.Read.new({
      availableRequests: permissions,
      rttMillis: 1n,
      data: Bindings.RemoteNodeData.Controllers.new({ page: { identities, next } }),
    }),
  });
  readRemoteNode
    .mockResolvedValueOnce(controllerPage([self, other], other))
    .mockResolvedValueOnce(controllerPage([last]));
  changeRemoteNode.mockResolvedValueOnce({
    type: "outcome",
    outcome: Bindings.ChangeRemoteNodeOutcome.Accepted.new({
      operation: {
        operationId: 1n,
        generationId: 1n,
        targetIdentityFingerprint: target,
        change: Bindings.RemoteNodeChange.RevokeController.new({
          controllerIdentityFingerprint: other,
        }),
        status: Bindings.RemoteChangeStatus.Applied.new(),
      },
    }),
  });
  const view = render(<RemoteManagementPanel target={target} runtime={accessRuntime} />);
  fireEvent.press(await view.findByRole("tab", { name: "Access" }));
  fireEvent.press(view.getByRole("button", { name: "Load devices" }));
  fireEvent.press(await view.findByRole("button", { name: "Load more devices" }));
  await view.findByText("Device 05050505");
  expect(view.getByText("This phone")).toBeTruthy();
  expect(readRemoteNode.mock.calls[2]?.[0].query).toEqual(
    Bindings.RemoteNodeQuery.Controllers.new({ after: other }),
  );
  fireEvent.press(view.getByRole("button", { name: "Remove access for 04040404" }));
  fireEvent.press(view.getByRole("button", { name: "Remove device access" }));
  await view.findByText("Refresh the node information to check device access.");
  expect(changeRemoteNode).toHaveBeenCalledTimes(1);
  expect(changeRemoteNode.mock.calls[0]?.[0].change).toEqual(
    Bindings.RemoteNodeChange.RevokeController.new({ controllerIdentityFingerprint: other }),
  );
  expect(view.getByRole("button", { name: "Refresh node information" })).toBeEnabled();
});
