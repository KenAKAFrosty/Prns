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
    bluetooth: { type: "ready" },
    controllerIdentityFingerprint: null,
    pairing: { type: "searching" },
    pairedTargets: [],
    activeOperation: null,
    failure: null,
  };
}

function fakeRuntime(overrides: Partial<DevelopmentRuntime> = {}): DevelopmentRuntime {
  return {
    startDevelopmentNode: jest.fn(async () => ({
      type: "started" as const,
      snapshot: snapshot(1n),
    })),
    readDevelopmentNodeSnapshot: jest.fn(async () => snapshot(1n)),
    initiateRemoteControlPairing: jest.fn(async () => ({ type: "busy" as const })),
    approveRemoteControlPairing: jest.fn(async () => ({ type: "busy" as const })),
    rejectRemoteControlPairing: jest.fn(async () => ({ type: "busy" as const })),
    describeRemoteControlTarget: jest.fn(async () => ({ type: "busy" as const })),
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
    expect(stopDevelopmentNode).toHaveBeenCalledTimes(1);
    const readsAfterClose = readDevelopmentNodeSnapshot.mock.calls.length;
    await new Promise((resolve) => setTimeout(resolve, 80));
    expect(readDevelopmentNodeSnapshot).toHaveBeenCalledTimes(readsAfterClose);
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
