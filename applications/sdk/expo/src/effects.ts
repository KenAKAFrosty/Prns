import { Data, Effect, type Scope } from "effect";
import type {
  DescribeRemoteControlTargetInput,
  DevelopmentNodeSnapshot,
  DevelopmentNodeStartInput,
  DevelopmentNodeStartOutcome,
  DevelopmentNodeStopOutcome,
  DevelopmentRuntime,
  InitiateRemoteControlPairingInput,
  RemoteControlDescribeOutcome,
  RemoteControlPairingCommandOutcome,
  RemoteControlPairingDecisionInput,
} from "./facade";

export type DevelopmentRuntimeOperationName =
  | "approvePairing"
  | "describeTarget"
  | "initiatePairing"
  | "rejectPairing"
  | "reset"
  | "snapshot"
  | "start"
  | "stop";

export class DevelopmentRuntimeOperationError extends Data.TaggedError(
  "DevelopmentRuntimeOperationError",
)<{
  readonly operation: DevelopmentRuntimeOperationName;
  readonly cause: unknown;
}> {}

export class DevelopmentRuntimeStartError extends Data.TaggedError("DevelopmentRuntimeStartError")<{
  readonly stage: Extract<DevelopmentNodeStartOutcome, { readonly type: "failed" }>["stage"];
  readonly detail: string;
}> {}

export class DevelopmentRuntimeStopError extends Data.TaggedError("DevelopmentRuntimeStopError")<{
  readonly stage: Extract<DevelopmentNodeStopOutcome, { readonly type: "failed" }>["stage"];
  readonly detail: string;
}> {}

export class DevelopmentRuntimeConfigurationError extends Data.TaggedError(
  "DevelopmentRuntimeConfigurationError",
)<{
  readonly detail: string;
}> {}

export type DevelopmentRuntimeFailure =
  | DevelopmentRuntimeConfigurationError
  | DevelopmentRuntimeOperationError
  | DevelopmentRuntimeStartError
  | DevelopmentRuntimeStopError;

export type EffectDevelopmentRuntime = {
  readonly startDevelopmentNode: (
    input: DevelopmentNodeStartInput,
  ) => Effect.Effect<DevelopmentNodeStartOutcome, DevelopmentRuntimeOperationError>;
  readonly readDevelopmentNodeSnapshot: Effect.Effect<
    DevelopmentNodeSnapshot,
    DevelopmentRuntimeOperationError
  >;
  readonly initiateRemoteControlPairing: (
    input: InitiateRemoteControlPairingInput,
  ) => Effect.Effect<RemoteControlPairingCommandOutcome, DevelopmentRuntimeOperationError>;
  readonly approveRemoteControlPairing: (
    input: RemoteControlPairingDecisionInput,
  ) => Effect.Effect<RemoteControlPairingCommandOutcome, DevelopmentRuntimeOperationError>;
  readonly rejectRemoteControlPairing: (
    input: RemoteControlPairingDecisionInput,
  ) => Effect.Effect<RemoteControlPairingCommandOutcome, DevelopmentRuntimeOperationError>;
  readonly describeRemoteControlTarget: (
    input: DescribeRemoteControlTargetInput,
  ) => Effect.Effect<RemoteControlDescribeOutcome, DevelopmentRuntimeOperationError>;
  readonly stopDevelopmentNode: Effect.Effect<
    DevelopmentNodeStopOutcome,
    DevelopmentRuntimeOperationError
  >;
  readonly resetDevelopmentData: Effect.Effect<
    DevelopmentNodeStopOutcome,
    DevelopmentRuntimeOperationError
  >;
};

export type DevelopmentRuntimeScopeOptions = {
  readonly refreshIntervalMillis?: number;
  readonly developmentTcpTarget?: DevelopmentNodeStartInput["developmentTcpTarget"];
  readonly shouldRefresh?: () => boolean;
  readonly onSnapshot: (snapshot: DevelopmentNodeSnapshot) => void;
  readonly onBackgroundFailure: (failure: DevelopmentRuntimeFailure) => void;
};

export type DevelopmentRuntimeSession = {
  readonly runtime: EffectDevelopmentRuntime;
  readonly initialSnapshot: DevelopmentNodeSnapshot;
};

const minimumRefreshIntervalMillis = 50;
const maximumRefreshIntervalMillis = 60_000;
const defaultRefreshIntervalMillis = 1_000;

export function makeEffectDevelopmentRuntime(
  runtime: DevelopmentRuntime,
): EffectDevelopmentRuntime {
  return {
    startDevelopmentNode: (input) =>
      runtimeCall("start", () => runtime.startDevelopmentNode(input)),
    readDevelopmentNodeSnapshot: runtimeCall("snapshot", runtime.readDevelopmentNodeSnapshot),
    initiateRemoteControlPairing: (input) =>
      runtimeCall("initiatePairing", () => runtime.initiateRemoteControlPairing(input)),
    approveRemoteControlPairing: (input) =>
      runtimeCall("approvePairing", () => runtime.approveRemoteControlPairing(input)),
    rejectRemoteControlPairing: (input) =>
      runtimeCall("rejectPairing", () => runtime.rejectRemoteControlPairing(input)),
    describeRemoteControlTarget: (input) =>
      runtimeCall("describeTarget", () => runtime.describeRemoteControlTarget(input)),
    stopDevelopmentNode: runtimeCall("stop", runtime.stopDevelopmentNode),
    resetDevelopmentData: runtimeCall("reset", runtime.resetDevelopmentData),
  };
}

export function scopedDevelopmentRuntime(
  runtime: DevelopmentRuntime,
  options: DevelopmentRuntimeScopeOptions,
): Effect.Effect<
  DevelopmentRuntimeSession,
  | DevelopmentRuntimeConfigurationError
  | DevelopmentRuntimeOperationError
  | DevelopmentRuntimeStartError,
  Scope.Scope
> {
  const intervalResult = refreshInterval(options.refreshIntervalMillis);
  if (intervalResult instanceof DevelopmentRuntimeConfigurationError) {
    return Effect.fail(intervalResult);
  }
  const effectRuntime = makeEffectDevelopmentRuntime(runtime);
  const release = releaseRuntime(effectRuntime, options.onBackgroundFailure);

  const acquire = Effect.gen(function* () {
    const outcome = yield* effectRuntime
      .startDevelopmentNode({
        developmentTcpTarget: options.developmentTcpTarget ?? null,
      })
      .pipe(Effect.catch((failure) => release.pipe(Effect.andThen(Effect.fail(failure)))));
    if (outcome.type === "failed") {
      yield* release;
      return yield* Effect.fail(
        new DevelopmentRuntimeStartError({
          stage: outcome.stage,
          detail: outcome.detail,
        }),
      );
    }
    return outcome.snapshot;
  });

  return Effect.gen(function* () {
    const initialSnapshot = yield* Effect.acquireRelease(acquire, () => release);
    let latestRevision = initialSnapshot.revision;
    yield* Effect.sync(() => options.onSnapshot(initialSnapshot));

    const refreshIteration = Effect.gen(function* () {
      yield* Effect.sleep(intervalResult);
      if (options.shouldRefresh?.() === false) {
        return;
      }
      const snapshot = yield* effectRuntime.readDevelopmentNodeSnapshot;
      if (snapshot.revision > latestRevision) {
        latestRevision = snapshot.revision;
        yield* Effect.sync(() => options.onSnapshot(snapshot));
      }
    }).pipe(Effect.catch((failure) => Effect.sync(() => options.onBackgroundFailure(failure))));
    yield* Effect.forkScoped(Effect.forever(refreshIteration));

    return { runtime: effectRuntime, initialSnapshot };
  });
}

function runtimeCall<Output>(
  operation: DevelopmentRuntimeOperationName,
  run: () => Promise<Output>,
): Effect.Effect<Output, DevelopmentRuntimeOperationError> {
  return Effect.tryPromise({
    try: run,
    catch: (cause) => new DevelopmentRuntimeOperationError({ operation, cause }),
  });
}

function refreshInterval(
  interval: number | undefined,
): number | DevelopmentRuntimeConfigurationError {
  const selected = interval ?? defaultRefreshIntervalMillis;
  if (
    !Number.isInteger(selected) ||
    selected < minimumRefreshIntervalMillis ||
    selected > maximumRefreshIntervalMillis
  ) {
    return new DevelopmentRuntimeConfigurationError({
      detail: `refreshIntervalMillis must be an integer from ${minimumRefreshIntervalMillis} through ${maximumRefreshIntervalMillis}`,
    });
  }
  return selected;
}

function releaseRuntime(
  runtime: EffectDevelopmentRuntime,
  onFailure: (failure: DevelopmentRuntimeFailure) => void,
): Effect.Effect<void> {
  return runtime.stopDevelopmentNode.pipe(
    Effect.matchEffect({
      onFailure: (failure) => Effect.sync(() => onFailure(failure)),
      onSuccess: (outcome) =>
        Effect.sync(() => {
          if (outcome.type === "failed") {
            onFailure(
              new DevelopmentRuntimeStopError({
                stage: outcome.stage,
                detail: outcome.detail,
              }),
            );
          }
        }),
    }),
  );
}
