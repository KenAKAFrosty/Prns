import { Effect } from "effect";
import {
  DevelopmentRuntimeConfigurationError,
  DevelopmentRuntimeOperationError,
  DevelopmentRuntimeStartError,
  DevelopmentRuntimeStopError,
  makeEffectDevelopmentRuntime,
  scopedDevelopmentRuntime,
} from "./effects";
import type { DevelopmentNodeSnapshot, DevelopmentRuntime } from "./facade";

function snapshot(revision: bigint): DevelopmentNodeSnapshot {
  return {
    contractFingerprint: "prns-app-native/foundation-1/test",
    revision,
    runtime: "running",
    primaryIdentity: { type: "missing" },
    localHost: { type: "stopped", lastStartFailure: null },
    lxmf: { state: "stopped", inboundOverflowCount: 0n },
    controllerIdentityFingerprint: null,
    pairing: { type: "searching" },
    pairedTargets: [],
    activeOperation: null,
    failure: null,
  };
}

function fakeRuntime(overrides: Partial<DevelopmentRuntime> = {}): DevelopmentRuntime {
  return {
    inspectDevelopmentIdentity: jest.fn(async () => ({ type: "missing" as const })),
    previewIdentityImport: jest.fn(async () => ({ type: "invalidLength" as const })),
    createGeneratedIdentity: jest.fn(async () => ({ type: "alreadyExists" as const })),
    createImportedIdentity: jest.fn(async () => ({ type: "alreadyExists" as const })),
    startDevelopmentNode: jest.fn(async () => ({
      type: "started" as const,
      snapshot: snapshot(1n),
    })),
    readDevelopmentNodeSnapshot: jest.fn(async () => snapshot(1n)),
    initiateRemoteControlPairing: jest.fn(async () => ({ type: "busy" as const })),
    approveRemoteControlPairing: jest.fn(async () => ({ type: "busy" as const })),
    rejectRemoteControlPairing: jest.fn(async () => ({ type: "busy" as const })),
    describeRemoteControlTarget: jest.fn(async () => ({ type: "busy" as const })),
    saveObservedDestination: jest.fn(async () => ({ type: "notObserved" as const })),
    createManualContact: jest.fn(async () => ({ type: "notFound" as const })),
    setContactAlias: jest.fn(async () => ({ type: "notFound" as const })),
    setContactPinned: jest.fn(async () => ({ type: "notFound" as const })),
    deleteContact: jest.fn(async () => ({ type: "notFound" as const })),
    getContact: jest.fn(async () => ({ type: "notFound" as const })),
    listContacts: jest.fn(async () => ({ type: "listed" as const, contacts: [] })),
    listLxmfPeers: jest.fn(async () => ({ type: "listed" as const, peers: [] })),
    listLxmfMessages: jest.fn(async () => ({ type: "listed" as const, messages: [] })),
    retryLxmfMessage: jest.fn(async () => ({ type: "notFound" as const })),
    cancelLxmfMessage: jest.fn(async () => ({ type: "notFound" as const })),
    announceLxmf: jest.fn(async () => ({ type: "announced" as const })),
    measureLxmfText: jest.fn(async () => ({
      type: "measured" as const,
      wireBytes: 113,
      remainingBytes: 318,
    })),
    sendDirectText: jest.fn(async () => ({ type: "accepted" as const, localRecordId: 1n })),
    stopDevelopmentNode: jest.fn(async () => ({ type: "stopped" as const })),
    resetDevelopmentData: jest.fn(async () => ({ type: "alreadyStopped" as const })),
    ...overrides,
  };
}

describe("Effect development runtime orchestration", () => {
  test("scopes one runtime, ignores stale revisions, and cancels sequential refresh", async () => {
    const revisions = [1n, 0n, 2n, 2n];
    let readIndex = 0;
    const readDevelopmentNodeSnapshot = jest.fn(async () => snapshot(revisions[readIndex++] ?? 2n));
    const stopDevelopmentNode = jest.fn(async () => ({ type: "stopped" as const }));
    const runtime = fakeRuntime({ readDevelopmentNodeSnapshot, stopDevelopmentNode });
    const observed: bigint[] = [];
    const failures: unknown[] = [];

    await Effect.runPromise(
      Effect.scoped(
        Effect.gen(function* () {
          yield* scopedDevelopmentRuntime(runtime, {
            refreshIntervalMillis: 50,
            onSnapshot: (current) => observed.push(current.revision),
            onBackgroundFailure: (failure) => failures.push(failure),
          });
          yield* Effect.sleep(190);
        }),
      ),
    );

    expect(observed).toEqual([1n, 2n]);
    expect(failures).toEqual([]);
    expect(runtime.startDevelopmentNode).toHaveBeenCalledWith({ developmentTcpTarget: null });
    expect(stopDevelopmentNode).toHaveBeenCalledTimes(1);
    const readsAfterClose = readDevelopmentNodeSnapshot.mock.calls.length;
    await new Promise((resolve) => setTimeout(resolve, 80));
    expect(readDevelopmentNodeSnapshot).toHaveBeenCalledTimes(readsAfterClose);
  });

  test("forwards the explicit development TCP target only through scoped startup", async () => {
    const runtime = fakeRuntime();

    await Effect.runPromise(
      Effect.scoped(
        scopedDevelopmentRuntime(runtime, {
          developmentTcpTarget: "192.0.2.1:4242",
          onSnapshot: () => undefined,
          onBackgroundFailure: () => undefined,
        }),
      ),
    );

    expect(runtime.startDevelopmentNode).toHaveBeenCalledWith({
      developmentTcpTarget: "192.0.2.1:4242",
    });
  });

  test("keeps the node owned while pausing snapshot reads outside consuming screens", async () => {
    let refreshActive = false;
    const readDevelopmentNodeSnapshot = jest.fn(async () => snapshot(2n));
    const stopDevelopmentNode = jest.fn(async () => ({ type: "stopped" as const }));
    const runtime = fakeRuntime({ readDevelopmentNodeSnapshot, stopDevelopmentNode });

    await Effect.runPromise(
      Effect.scoped(
        Effect.gen(function* () {
          yield* scopedDevelopmentRuntime(runtime, {
            refreshIntervalMillis: 50,
            shouldRefresh: () => refreshActive,
            onSnapshot: () => undefined,
            onBackgroundFailure: () => undefined,
          });
          yield* Effect.sleep(120);
          expect(readDevelopmentNodeSnapshot).not.toHaveBeenCalled();
          refreshActive = true;
          yield* Effect.sleep(120);
          expect(readDevelopmentNodeSnapshot).toHaveBeenCalled();
        }),
      ),
    );

    expect(stopDevelopmentNode).toHaveBeenCalledTimes(1);
  });

  test("stops after a typed startup rejection before failing acquisition", async () => {
    const stopDevelopmentNode = jest.fn(async () => ({ type: "stopped" as const }));
    const runtime = fakeRuntime({
      startDevelopmentNode: jest.fn(async () => ({
        type: "failed" as const,
        stage: "runtime" as const,
        detail: "worker did not start",
      })),
      stopDevelopmentNode,
    });

    const failure = await Effect.runPromise(
      Effect.flip(
        Effect.scoped(
          scopedDevelopmentRuntime(runtime, {
            onSnapshot: () => undefined,
            onBackgroundFailure: () => undefined,
          }),
        ),
      ),
    );

    expect(failure).toBeInstanceOf(DevelopmentRuntimeStartError);
    expect(stopDevelopmentNode).toHaveBeenCalledTimes(1);
  });

  test("maps rejected native promises to operation-specific typed errors", async () => {
    const runtime = makeEffectDevelopmentRuntime(
      fakeRuntime({
        readDevelopmentNodeSnapshot: jest.fn(async () => {
          throw new Error("bridge detached");
        }),
      }),
    );

    const failure = await Effect.runPromise(Effect.flip(runtime.readDevelopmentNodeSnapshot));

    expect(failure).toBeInstanceOf(DevelopmentRuntimeOperationError);
    expect(failure.operation).toBe("snapshot");
  });

  test("reports a transient snapshot failure and continues bounded refresh", async () => {
    let readCount = 0;
    const readDevelopmentNodeSnapshot = jest.fn(async () => {
      readCount += 1;
      if (readCount === 1) {
        throw new Error("temporary bridge interruption");
      }
      return snapshot(2n);
    });
    const observed: bigint[] = [];
    const failures: unknown[] = [];

    await Effect.runPromise(
      Effect.scoped(
        Effect.gen(function* () {
          yield* scopedDevelopmentRuntime(fakeRuntime({ readDevelopmentNodeSnapshot }), {
            refreshIntervalMillis: 50,
            onSnapshot: (current) => observed.push(current.revision),
            onBackgroundFailure: (failure) => failures.push(failure),
          });
          yield* Effect.sleep(140);
        }),
      ),
    );

    expect(readDevelopmentNodeSnapshot).toHaveBeenCalledTimes(2);
    expect(observed).toEqual([1n, 2n]);
    expect(failures).toHaveLength(1);
    expect(failures[0]).toBeInstanceOf(DevelopmentRuntimeOperationError);
  });

  test("rejects an unbounded refresh cadence before starting native", async () => {
    const runtime = fakeRuntime();

    const failure = await Effect.runPromise(
      Effect.flip(
        Effect.scoped(
          scopedDevelopmentRuntime(runtime, {
            refreshIntervalMillis: 1,
            onSnapshot: () => undefined,
            onBackgroundFailure: () => undefined,
          }),
        ),
      ),
    );

    expect(failure).toBeInstanceOf(DevelopmentRuntimeConfigurationError);
    expect(runtime.startDevelopmentNode).not.toHaveBeenCalled();
  });

  test("reports an incomplete final stop through the required failure sink", async () => {
    const failures: unknown[] = [];
    const runtime = fakeRuntime({
      stopDevelopmentNode: jest.fn(async () => ({
        type: "failed" as const,
        stage: "worker" as const,
        detail: "worker did not join",
      })),
    });

    await Effect.runPromise(
      Effect.scoped(
        scopedDevelopmentRuntime(runtime, {
          onSnapshot: () => undefined,
          onBackgroundFailure: (failure) => failures.push(failure),
        }),
      ),
    );

    expect(failures).toHaveLength(1);
    expect(failures[0]).toBeInstanceOf(DevelopmentRuntimeStopError);
  });
});
