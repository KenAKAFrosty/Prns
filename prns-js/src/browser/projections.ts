import { Tag, from, match } from "../casework.js";
import type { Tag as Tagged } from "../casework.js";
import type {
  HostSnapshot,
  IdentityHash,
  InterfaceSnapshot,
  LifecycleState,
  LinkId,
  PrnsLimits,
  RouteSnapshot,
} from "../contract.js";
import type { PrnsDiagnosticEvent } from "./events.js";

declare const projectionRevisionBrand: unique symbol;

export type ProjectionRevision = bigint & {
  readonly [projectionRevisionBrand]: "ProjectionRevision";
};

export type PrnsView =
  | Tagged<"Lifecycle">
  | Tagged<"Interfaces">
  | Tagged<"Routes">
  | Tagged<"Links">
  | Tagged<"Diagnostics", { readonly maximumEvents: number }>;

export type ActiveLinkSnapshot = {
  readonly linkId: LinkId;
  readonly rttMillis: number;
  readonly peerIdentity?: IdentityHash;
};

export type PrnsProjectionValue<View extends PrnsView> =
  View extends Tagged<"Lifecycle"> ? LifecycleState
    : View extends Tagged<"Interfaces"> ? readonly InterfaceSnapshot[]
    : View extends Tagged<"Routes"> ? readonly RouteSnapshot[]
    : View extends Tagged<"Links"> ? readonly ActiveLinkSnapshot[]
    : View extends Tagged<"Diagnostics", unknown> ? readonly PrnsDiagnosticEvent[]
    : never;

export type PrnsProjectionSnapshot<Value> = {
  readonly revision: ProjectionRevision;
  readonly value: Value;
};

export type ProjectionUnavailableLifecycle = Exclude<
  LifecycleState,
  Tagged<"Running">
>;

export type ProjectionSynchronization<Value> =
  | Tagged<"Synchronized", PrnsProjectionSnapshot<Value>>
  | Tagged<"Busy">
  | Tagged<"Unavailable", { readonly lifecycle: ProjectionUnavailableLifecycle }>;

export interface PrnsProjection<Value> {
  latest(): PrnsProjectionSnapshot<Value>;
  synchronize(): Promise<ProjectionSynchronization<Value>>;
  subscribe(changed: () => void): () => void;
}

export class PrnsProjectionCapacityError extends RangeError {
  readonly maximumEvents: number;
  readonly configuredMaximum: number;

  constructor(maximumEvents: number, configuredMaximum: number) {
    super(
      `diagnostic projection capacity ${maximumEvents} must be a positive integer no greater than ${configuredMaximum}`,
    );
    this.name = "PrnsProjectionCapacityError";
    this.maximumEvents = maximumEvents;
    this.configuredMaximum = configuredMaximum;
  }
}

export const { MakeTag: prnsView } = from<PrnsView>();

type ProjectionKey = PrnsView["tag"] | `Diagnostics:${number}`;
type ReplicatedProjectionKey = Exclude<PrnsView["tag"], "Diagnostics">;

type ProjectionEntry = {
  readonly key: ProjectionKey;
  readonly view: PrnsView;
  readonly listeners: Set<() => void>;
  readonly projection: PrnsProjection<unknown>;
  snapshot: PrnsProjectionSnapshot<unknown>;
};

type ProjectionStoreHooks = {
  readonly observed?: (view: PrnsView) => void;
  readonly unobserved?: (view: PrnsView) => void;
  readonly synchronize?: (
    view: PrnsView,
  ) => Promise<ProjectionSynchronization<unknown>>;
  readonly diagnosticCapacityChanged?: (maximumEvents: number) => void;
};

export class PrnsProjectionStore {
  readonly #maximumDiagnostics: number;
  readonly #hooks: ProjectionStoreHooks;
  readonly #entries = new Map<ProjectionKey, ProjectionEntry>();
  readonly #values = new Map<
    ReplicatedProjectionKey,
    PrnsProjectionSnapshot<unknown>
  >();
  readonly #acceptedRevisions = new Map<ReplicatedProjectionKey, ProjectionRevision>();
  readonly #diagnostics: PrnsDiagnosticEvent[] = [];
  readonly #dirty = new Set<ProjectionKey>();
  #revision: ProjectionRevision;
  #acceptedDiagnosticRevision: ProjectionRevision | undefined;
  #diagnosticCapacity = 0;
  #notificationScheduled = false;

  constructor(
    snapshot: HostSnapshot,
    lifecycle: LifecycleState,
    maximumDiagnostics: number,
    hooks: ProjectionStoreHooks = {},
  ) {
    const revision = asProjectionRevision(snapshot.revision);
    this.#revision = revision;
    this.#maximumDiagnostics = maximumDiagnostics;
    this.#hooks = hooks;
    this.#seed("Lifecycle", lifecycle, revision);
    this.#seed("Interfaces", Object.freeze([...snapshot.interfaces]), revision);
    this.#seed("Routes", Object.freeze([...snapshot.routes]), revision);
    this.#values.set("Links", {
      revision,
      value: Object.freeze([]),
    });
  }

  projection<View extends PrnsView>(
    view: View,
  ): PrnsProjection<PrnsProjectionValue<View>> {
    const key = projectionKey(view, this.#maximumDiagnostics);
    let entry = this.#entries.get(key);
    if (entry === undefined) {
      const listeners = new Set<() => void>();
      let stable: ProjectionEntry;
      const projection: PrnsProjection<unknown> = {
        latest: () => stable.snapshot,
        synchronize: () => this.#synchronize(stable),
        subscribe: (changed) => {
          const first = stable.listeners.size === 0;
          const listener = () => changed();
          stable.listeners.add(listener);
          if (first) {
            this.#hooks.observed?.(stable.view);
            if (stable.view.tag === "Diagnostics") {
              this.#diagnosticSubscriptionsChanged();
            }
          }
          let subscribed = true;
          return () => {
            if (!subscribed) {
              return;
            }
            subscribed = false;
            stable.listeners.delete(listener);
            if (stable.listeners.size !== 0) {
              return;
            }
            this.#hooks.unobserved?.(stable.view);
            if (stable.view.tag === "Diagnostics") {
              this.#diagnosticSubscriptionsChanged();
            }
          };
        },
      };
      stable = {
        key,
        view,
        listeners,
        snapshot: this.#snapshotFor(view),
        projection,
      };
      entry = stable;
      this.#entries.set(key, entry);
    }
    return entry.projection as PrnsProjection<PrnsProjectionValue<View>>;
  }

  replaceLifecycle(
    lifecycle: LifecycleState,
    revision?: ProjectionRevision,
  ): void {
    this.#replaceReplicated(
      "Lifecycle",
      lifecycle,
      equalLifecycle,
      revision,
    );
  }

  replaceHostSnapshot(snapshot: HostSnapshot): void {
    const revision = asProjectionRevision(snapshot.revision);
    this.replaceInterfaces(snapshot.interfaces, revision);
    this.replaceRoutes(snapshot.routes, revision);
  }

  replaceInterfaces(
    interfaces: readonly InterfaceSnapshot[],
    revision?: ProjectionRevision,
  ): void {
    this.#replaceReplicated(
      "Interfaces",
      Object.freeze([...interfaces]),
      equalInterfaces,
      revision,
    );
  }

  replaceRoutes(
    routes: readonly RouteSnapshot[],
    revision?: ProjectionRevision,
  ): void {
    this.#replaceReplicated(
      "Routes",
      Object.freeze([...routes]),
      equalRoutes,
      revision,
    );
  }

  replaceLinks(
    links: readonly ActiveLinkSnapshot[],
    revision?: ProjectionRevision,
  ): void {
    this.#replaceReplicated(
      "Links",
      Object.freeze([...links]),
      equalLinks,
      revision,
    );
  }

  publishDiagnostic(event: PrnsDiagnosticEvent): void {
    if (this.#diagnosticCapacity === 0) {
      return;
    }
    this.#diagnostics.push(event);
    if (this.#diagnostics.length > this.#diagnosticCapacity) {
      this.#diagnostics.splice(
        0,
        this.#diagnostics.length - this.#diagnosticCapacity,
      );
    }
    this.#refreshDiagnosticEntries();
  }

  replaceDiagnostics(
    events: readonly PrnsDiagnosticEvent[],
    revision?: ProjectionRevision,
  ): void {
    if (!this.#acceptDiagnosticRevision(revision)) {
      return;
    }
    this.#diagnostics.length = 0;
    if (this.#diagnosticCapacity > 0) {
      this.#diagnostics.push(...events.slice(-this.#diagnosticCapacity));
    }
    this.#refreshDiagnosticEntries(revision);
  }

  appendDiagnostics(
    dropped: number,
    events: readonly PrnsDiagnosticEvent[],
    revision?: ProjectionRevision,
  ): void {
    if (!Number.isSafeInteger(dropped) || dropped < 0) {
      throw new RangeError("diagnostic delta drop count must be a non-negative safe integer");
    }
    if (!this.#acceptDiagnosticRevision(revision)) {
      return;
    }
    if (this.#diagnosticCapacity === 0) {
      return;
    }
    if (dropped > 0) {
      this.#diagnostics.splice(0, Math.min(dropped, this.#diagnostics.length));
    }
    this.#diagnostics.push(...events);
    if (this.#diagnostics.length > this.#diagnosticCapacity) {
      this.#diagnostics.splice(
        0,
        this.#diagnostics.length - this.#diagnosticCapacity,
      );
    }
    this.#refreshDiagnosticEntries(revision);
  }

  get diagnosticCapacity(): number {
    return this.#diagnosticCapacity;
  }

  #seed(
    key: ReplicatedProjectionKey,
    value: unknown,
    revision: ProjectionRevision,
  ): void {
    this.#values.set(key, { revision, value });
    this.#acceptedRevisions.set(key, revision);
  }

  #snapshotFor(view: PrnsView): PrnsProjectionSnapshot<unknown> {
    if (view.tag !== "Diagnostics") {
      const snapshot = this.#values.get(view.tag);
      if (snapshot === undefined) {
        throw new Error(`projection state ${view.tag} is unavailable`);
      }
      return snapshot;
    }
    return {
      revision: this.#revision,
      value: Object.freeze(
        this.#diagnostics.slice(-view.data.maximumEvents),
      ),
    };
  }

  async #synchronize(
    entry: ProjectionEntry,
  ): Promise<ProjectionSynchronization<unknown>> {
    const synchronize = this.#hooks.synchronize;
    if (synchronize === undefined) {
      return Tag("Synchronized", entry.snapshot);
    }
    const outcome = await synchronize(entry.view);
    if (outcome.tag !== "Synchronized") {
      return outcome;
    }
    this.#applySynchronized(entry.view, outcome.data);
    return Tag("Synchronized", entry.snapshot);
  }

  #applySynchronized(
    view: PrnsView,
    snapshot: PrnsProjectionSnapshot<unknown>,
  ): void {
    match(view, {
      Lifecycle: () => this.replaceLifecycle(
        snapshot.value as LifecycleState,
        snapshot.revision,
      ),
      Interfaces: () => this.replaceInterfaces(
        snapshot.value as readonly InterfaceSnapshot[],
        snapshot.revision,
      ),
      Routes: () => this.replaceRoutes(
        snapshot.value as readonly RouteSnapshot[],
        snapshot.revision,
      ),
      Links: () => this.replaceLinks(
        snapshot.value as readonly ActiveLinkSnapshot[],
        snapshot.revision,
      ),
      Diagnostics: () => this.replaceDiagnostics(
        snapshot.value as readonly PrnsDiagnosticEvent[],
        snapshot.revision,
      ),
    });
  }

  #replaceReplicated<Value>(
    key: ReplicatedProjectionKey,
    value: Value,
    equal: (left: Value, right: Value) => boolean,
    receivedRevision?: ProjectionRevision,
  ): void {
    const current = this.#values.get(key) as
      | PrnsProjectionSnapshot<Value>
      | undefined;
    if (current === undefined) {
      throw new Error(`projection state ${key} is unavailable`);
    }
    const accepted = this.#acceptedRevisions.get(key);
    if (
      receivedRevision !== undefined &&
      accepted !== undefined &&
      receivedRevision <= accepted
    ) {
      return;
    }
    if (receivedRevision !== undefined) {
      this.#acceptedRevisions.set(key, receivedRevision);
      if (receivedRevision > this.#revision) {
        this.#revision = receivedRevision;
      }
    }
    if (equal(current.value, value)) {
      return;
    }
    const revision = receivedRevision ?? this.#nextRevision();
    if (receivedRevision === undefined) {
      this.#acceptedRevisions.set(key, revision);
    }
    const snapshot = { revision, value };
    this.#values.set(key, snapshot);
    const entry = this.#entries.get(key);
    if (entry === undefined) {
      return;
    }
    entry.snapshot = snapshot;
    this.#dirty.add(key);
    this.#scheduleNotifications();
  }

  #nextRevision(): ProjectionRevision {
    this.#revision = asProjectionRevision(this.#revision + 1n);
    return this.#revision;
  }

  #acceptDiagnosticRevision(revision: ProjectionRevision | undefined): boolean {
    if (revision === undefined) {
      return true;
    }
    if (
      this.#acceptedDiagnosticRevision !== undefined &&
      revision <= this.#acceptedDiagnosticRevision
    ) {
      return false;
    }
    this.#acceptedDiagnosticRevision = revision;
    if (revision > this.#revision) {
      this.#revision = revision;
    }
    return true;
  }

  #refreshDiagnosticEntries(
    receivedRevision?: ProjectionRevision,
  ): void {
    let revision = receivedRevision;
    for (const entry of this.#entries.values()) {
      if (entry.view.tag !== "Diagnostics") {
        continue;
      }
      const value = Object.freeze(
        this.#diagnostics.slice(-entry.view.data.maximumEvents),
      );
      if (equalDiagnostics(entry.snapshot.value as readonly PrnsDiagnosticEvent[], value)) {
        continue;
      }
      if (revision === undefined) {
        revision = this.#nextRevision();
        this.#acceptedDiagnosticRevision = revision;
      }
      entry.snapshot = { revision, value };
      this.#dirty.add(entry.key);
    }
    this.#scheduleNotifications();
  }

  #diagnosticSubscriptionsChanged(): void {
    let maximum = 0;
    for (const entry of this.#entries.values()) {
      if (entry.view.tag === "Diagnostics" && entry.listeners.size > 0) {
        maximum = Math.max(maximum, entry.view.data.maximumEvents);
      }
    }
    if (maximum === this.#diagnosticCapacity) {
      return;
    }
    this.#diagnosticCapacity = maximum;
    if (maximum === 0) {
      this.#diagnostics.length = 0;
      this.#refreshDiagnosticEntries();
    } else if (this.#diagnostics.length > maximum) {
      this.#diagnostics.splice(0, this.#diagnostics.length - maximum);
      this.#refreshDiagnosticEntries();
    }
    this.#hooks.diagnosticCapacityChanged?.(maximum);
  }

  #scheduleNotifications(): void {
    if (this.#notificationScheduled || this.#dirty.size === 0) {
      return;
    }
    this.#notificationScheduled = true;
    queueMicrotask(() => {
      this.#notificationScheduled = false;
      const dirty = [...this.#dirty];
      this.#dirty.clear();
      for (const key of dirty) {
        const entry = this.#entries.get(key);
        if (entry === undefined) {
          continue;
        }
        for (const listener of entry.listeners) {
          listener();
        }
      }
    });
  }
}

export function asProjectionRevision(revision: bigint): ProjectionRevision {
  return revision as ProjectionRevision;
}

function projectionKey(
  view: PrnsView,
  maximumDiagnostics: number,
): ProjectionKey {
  if (view.tag !== "Diagnostics") {
    return view.tag;
  }
  const maximumEvents = view.data.maximumEvents;
  if (
    !Number.isSafeInteger(maximumEvents) ||
    maximumEvents <= 0 ||
    maximumEvents > maximumDiagnostics
  ) {
    throw new PrnsProjectionCapacityError(maximumEvents, maximumDiagnostics);
  }
  return `Diagnostics:${maximumEvents}`;
}

function equalLifecycle(left: LifecycleState, right: LifecycleState): boolean {
  return match(left, {
    Starting: () => right.tag === "Starting",
    Running: () => right.tag === "Running",
    Stopping: () => right.tag === "Stopping",
    Stopped: ({ reason }) =>
      right.tag === "Stopped" && right.data.reason === reason,
    Failed: (failure) => {
      if (right.tag !== "Failed" || right.data.cause !== failure.cause) {
        return false;
      }
      if (failure.cause === "EventBackpressureExceeded") {
        return right.data.cause === "EventBackpressureExceeded" &&
          right.data.rejectedEventBytes === failure.rejectedEventBytes &&
          equalLimits(right.data.limits, failure.limits);
      }
      return right.data.cause !== "EventBackpressureExceeded" &&
        right.data.detail === failure.detail;
    },
  });
}

function equalLimits(left: PrnsLimits, right: PrnsLimits): boolean {
  return left.pendingCommands === right.pendingCommands &&
    left.applicationEvents === right.applicationEvents &&
    left.retainedEventBytes === right.retainedEventBytes &&
    left.diagnostics === right.diagnostics;
}

function equalInterfaces(
  left: readonly InterfaceSnapshot[],
  right: readonly InterfaceSnapshot[],
): boolean {
  if (left.length !== right.length) {
    return false;
  }
  for (let index = 0; index < left.length; index += 1) {
    const a = left[index];
    const b = right[index];
    if (
      a === undefined ||
      b === undefined ||
      !equalBytes(a.interfaceId, b.interfaceId) ||
      a.name !== b.name ||
      a.kind !== b.kind ||
      a.health !== b.health ||
      a.failureDetail !== b.failureDetail ||
      a.rxBytes !== b.rxBytes ||
      a.txBytes !== b.txBytes ||
      a.rxBps !== b.rxBps ||
      a.txBps !== b.txBps ||
      a.routeCount !== b.routeCount ||
      a.linkCount !== b.linkCount ||
      a.transportedLinkCount !== b.transportedLinkCount
    ) {
      return false;
    }
  }
  return true;
}

function equalRoutes(
  left: readonly RouteSnapshot[],
  right: readonly RouteSnapshot[],
): boolean {
  if (left.length !== right.length) {
    return false;
  }
  for (let index = 0; index < left.length; index += 1) {
    const a = left[index];
    const b = right[index];
    if (
      a === undefined ||
      b === undefined ||
      !equalBytes(a.destination, b.destination) ||
      a.hops !== b.hops ||
      !equalOptionalBytes(a.viaIdentity, b.viaIdentity) ||
      !equalBytes(a.interfaceId, b.interfaceId) ||
      a.learnedAtMillis !== b.learnedAtMillis ||
      a.lastRouteActivityAtMillis !== b.lastRouteActivityAtMillis ||
      a.expiresAtMillis !== b.expiresAtMillis
    ) {
      return false;
    }
  }
  return true;
}

function equalLinks(
  left: readonly ActiveLinkSnapshot[],
  right: readonly ActiveLinkSnapshot[],
): boolean {
  if (left.length !== right.length) {
    return false;
  }
  for (let index = 0; index < left.length; index += 1) {
    const a = left[index];
    const b = right[index];
    if (
      a === undefined ||
      b === undefined ||
      !equalBytes(a.linkId, b.linkId) ||
      a.rttMillis !== b.rttMillis ||
      !equalOptionalBytes(a.peerIdentity, b.peerIdentity)
    ) {
      return false;
    }
  }
  return true;
}

function equalDiagnostics(
  left: readonly PrnsDiagnosticEvent[],
  right: readonly PrnsDiagnosticEvent[],
): boolean {
  return left.length === right.length && left.every(
    (event, index) => event === right[index],
  );
}

function equalOptionalBytes(
  left: Uint8Array | undefined,
  right: Uint8Array | undefined,
): boolean {
  if (left === undefined || right === undefined) {
    return left === right;
  }
  return equalBytes(left, right);
}

function equalBytes(left: Uint8Array, right: Uint8Array): boolean {
  return left.length === right.length && left.every(
    (value, index) => value === right[index],
  );
}
