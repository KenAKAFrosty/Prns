import type {
  DevelopmentNodeSnapshot,
  RemoteControlDescribeOutcome,
  RemoteControlAnnounceOutcome,
} from "@prns-internal/expo";
import type { RuntimeCommandResult } from "@/native/development-runtime-context";
import { act, fireEvent, render, waitFor } from "@testing-library/react-native";
import { destinationHash, identityHash } from "personal-rns/contract";
import type { ReactNode } from "react";

import { ManagedNodeScreen, announcementStatusMessage } from "./managed-node-screen";

const target = {
  targetIdentityFingerprint: identityHash(new Uint8Array(16).fill(0x44)),
  destination: destinationHash(new Uint8Array(16).fill(0x45)),
  controllerIdentityFingerprint: identityHash(new Uint8Array(16).fill(0x46)),
  permittedRequests: ["describe", "announceSelf"] as const,
};
const mockSnapshot: DevelopmentNodeSnapshot = {
  contractFingerprint: "test",
  revision: 1n,
  generationId: 1n,
  runtime: "running",
  primaryIdentity: { type: "missing" },
  localHost: { type: "stopped", lastStartFailure: null },
  lxmf: { state: "stopped", inboundOverflowCount: 0n },
  controllerIdentityFingerprint: null,
  pairing: { type: "searching" },
  pairingCandidates: [],
  pairedTargets: [target],
  activeOperation: null,
  lastAnnouncement: null,
  failure: null,
};
const mockDescribe = jest.fn<
  Promise<{ type: "outcome"; outcome: RemoteControlDescribeOutcome }>,
  []
>();
const mockAnnounce = jest.fn(
  async (): Promise<RuntimeCommandResult<RemoteControlAnnounceOutcome>> => {
    const operation = {
      operationId: 7n,
      targetIdentityFingerprint: target.targetIdentityFingerprint,
      status: { type: "pending" as const },
    };
    mockRuntime.snapshot = {
      ...mockSnapshot,
      revision: 2n,
      lastAnnouncement: operation,
      activeOperation: { kind: "announceSelf", startedAtMillis: 0n },
    };
    return {
      type: "outcome" as const,
      outcome: { type: "accepted" as const, operation, snapshot: mockRuntime.snapshot },
    };
  },
);
const mockRuntime = {
  phase: "ready",
  availability: { type: "available", platform: "ios" },
  snapshot: mockSnapshot,
  describeTarget: mockDescribe,
  announceTarget: mockAnnounce,
  refreshSnapshot: jest.fn(
    async (): Promise<RuntimeCommandResult<DevelopmentNodeSnapshot>> => ({
      type: "operationFailure",
      detail: "recovery failed",
    }),
  ),
};

jest.mock("@/native/development-runtime-context", () => ({
  useDevelopmentRuntime: () => mockRuntime,
}));
jest.mock("expo-router", () => ({
  Link: ({ children }: { readonly children: ReactNode }) => children,
  useLocalSearchParams: () => ({ nodeId: "44444444444444444444444444444444" }),
}));

beforeEach(() => {
  jest.clearAllMocks();
  mockRuntime.snapshot = mockSnapshot;
  mockDescribe.mockResolvedValue({
    type: "outcome",
    outcome: {
      type: "described",
      target,
      availableRequests: ["describe", "announceSelf"],
      rttMillis: 3n,
      snapshot: mockSnapshot,
    },
  });
});

test("offers address sharing only after a successful Describe authorizes it", async () => {
  const screen = render(<ManagedNodeScreen />);
  expect(screen.queryByRole("button", { name: "Share node address" })).toBeNull();
  fireEvent.press(screen.getByRole("button", { name: "Check node connection" }));
  expect(await screen.findByRole("button", { name: "Share node address" })).toBeTruthy();
  expect(mockAnnounce).not.toHaveBeenCalled();
});

test("cached permissions do not override the live target's restricted actions", async () => {
  mockDescribe.mockResolvedValue({
    type: "outcome",
    outcome: {
      type: "described",
      target,
      availableRequests: ["describe"],
      rttMillis: 3n,
      snapshot: mockSnapshot,
    },
  });
  const screen = render(<ManagedNodeScreen />);
  fireEvent.press(screen.getByRole("button", { name: "Check node connection" }));
  await screen.findByText("Node reached");
  expect(screen.queryByRole("button", { name: "Share node address" })).toBeNull();
});

test("a native generation change invalidates the earlier Describe evidence", async () => {
  const screen = render(<ManagedNodeScreen />);
  fireEvent.press(screen.getByRole("button", { name: "Check node connection" }));
  await screen.findByRole("button", { name: "Share node address" });
  mockRuntime.snapshot = { ...mockSnapshot, generationId: 2n };
  screen.rerender(<ManagedNodeScreen />);
  expect(screen.queryByRole("button", { name: "Share node address" })).toBeNull();
});

test("removing current saved permission hides the action even after Describe", async () => {
  const screen = render(<ManagedNodeScreen />);
  fireEvent.press(screen.getByRole("button", { name: "Check node connection" }));
  await screen.findByRole("button", { name: "Share node address" });
  mockRuntime.snapshot = {
    ...mockSnapshot,
    pairedTargets: [{ ...target, permittedRequests: ["describe"] }],
  };
  screen.rerender(<ManagedNodeScreen />);
  expect(screen.queryByRole("button", { name: "Share node address" })).toBeNull();
});

test.each(["stopping", "stopped", "failed"] as const)(
  "a %s generation cannot use retained Describe evidence",
  async (runtime) => {
    const screen = render(<ManagedNodeScreen />);
    fireEvent.press(screen.getByRole("button", { name: "Check node connection" }));
    await screen.findByRole("button", { name: "Share node address" });
    mockRuntime.snapshot = { ...mockSnapshot, runtime };
    screen.rerender(<ManagedNodeScreen />);
    expect(screen.queryByRole("button", { name: "Share node address" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Check node connection" })).toBeNull();
    expect(screen.queryByText("Node reached")).toBeNull();
    expect(
      screen.getByText(`This device's node is ${runtime === "failed" ? "unavailable" : runtime}`),
    ).toBeTruthy();
    expect(screen.getByText("Back to Nodes")).toBeTruthy();
    expect(screen.getByText(/return to Nodes to check this device's status\./iu)).toBeTruthy();
    expect(screen.queryByText(/start it|start this device|starting it/iu)).toBeNull();
    expect(mockAnnounce).not.toHaveBeenCalled();
  },
);

test("a lost submission reply cannot reuse an old result or unlock another submission", async () => {
  mockRuntime.snapshot = {
    ...mockSnapshot,
    lastAnnouncement: {
      operationId: 6n,
      targetIdentityFingerprint: target.targetIdentityFingerprint,
      status: { type: "announced", rttMillis: 4n },
    },
  };
  const screen = render(<ManagedNodeScreen />);
  fireEvent.press(screen.getByRole("button", { name: "Check node connection" }));
  await screen.findByRole("button", { name: "Share node address" });
  mockAnnounce.mockResolvedValueOnce({ type: "operationFailure", detail: "reply lost" });
  let settleRefresh: ((result: RuntimeCommandResult<DevelopmentNodeSnapshot>) => void) | undefined;
  mockRuntime.refreshSnapshot.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        settleRefresh = resolve;
      }),
  );
  fireEvent.press(screen.getByRole("button", { name: "Share node address" }));
  await waitFor(() => expect(mockRuntime.refreshSnapshot).toHaveBeenCalledTimes(1));
  expect(screen.getByRole("button", { name: "Sharing…" })).toBeDisabled();
  expect(screen.getByText("Previous address sharing")).toBeTruthy();
  await act(async () => {
    settleRefresh?.({ type: "operationFailure", detail: "refresh failed" });
  });
  expect(screen.getByText(/The node may have shared its address/)).toBeTruthy();
  expect(screen.getByRole("button", { name: "Share node address" })).toBeDisabled();
  fireEvent.press(screen.getByRole("button", { name: "Share node address" }));
  expect(mockAnnounce).toHaveBeenCalledTimes(1);
});

test("pending and unknown results survive remount without another submission", async () => {
  const screen = render(<ManagedNodeScreen />);
  fireEvent.press(screen.getByRole("button", { name: "Check node connection" }));
  fireEvent.press(await screen.findByRole("button", { name: "Share node address" }));
  await waitFor(() => expect(mockAnnounce).toHaveBeenCalledTimes(1));
  expect(await screen.findByText("Waiting for the node to confirm address sharing…")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Sharing…" })).toBeDisabled();
  screen.unmount();
  const reopened = render(<ManagedNodeScreen />);
  expect(reopened.getByText("Waiting for the node to confirm address sharing…")).toBeTruthy();
  mockRuntime.snapshot = {
    ...mockRuntime.snapshot,
    activeOperation: null,
    lastAnnouncement: {
      operationId: 7n,
      targetIdentityFingerprint: target.targetIdentityFingerprint,
      status: { type: "outcomeUnknown", reason: "timeout" },
    },
  };
  reopened.rerender(<ManagedNodeScreen />);
  expect(reopened.getByText(/The node may have shared its address/)).toBeTruthy();
  expect(mockAnnounce).toHaveBeenCalledTimes(1);
});

test("a retained confirmed result is visible after remount", () => {
  mockRuntime.snapshot = {
    ...mockSnapshot,
    lastAnnouncement: {
      operationId: 7n,
      targetIdentityFingerprint: target.targetIdentityFingerprint,
      status: { type: "announced", rttMillis: 4n },
    },
  };
  const screen = render(<ManagedNodeScreen />);
  expect(screen.getByText("The node confirmed that it shared its address.")).toBeTruthy();
  expect(screen.getByText("4 ms")).toBeTruthy();
  expect(mockAnnounce).not.toHaveBeenCalled();
});

test("definite target failures and ambiguous delivery have different guidance", () => {
  const messages = [
    announcementStatusMessage({ type: "unavailable" }),
    announcementStatusMessage({ type: "rejected" }),
    announcementStatusMessage({ type: "writeFailed" }),
    announcementStatusMessage({ type: "outcomeUnknown", reason: "timeout" }),
  ];
  expect(new Set(messages).size).toBe(4);
  expect(messages.join(" ")).not.toMatch(/RemoteControl|E290|signed|upstream/);
});
