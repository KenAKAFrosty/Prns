import { Tag, match_into } from "../casework.js";
import {
  DESTINATION_HASH_LENGTH,
  DIAGNOSTIC_EVENT_KIND_CODES,
  IDENTITY_HASH_LENGTH,
  INTERFACE_ID_LENGTH,
  LINK_ID_LENGTH,
  isInterfaceHealth,
  isInterfaceKind,
} from "../contract.js";
import type {
  InterfaceSnapshot,
  LifecycleState,
  PrnsLimits,
  RouteSnapshot,
} from "../contract.js";
import {
  field,
  nonNegativeBigIntField,
  record,
  stringField,
} from "./decoding.js";
import type { PrnsDiagnosticEvent } from "./events.js";
import {
  asProjectionRevision,
  prnsView,
} from "./projections.js";
import type {
  ActiveLinkSnapshot,
  PrnsProjectionSnapshot,
  PrnsView,
  ProjectionSynchronization,
} from "./projections.js";
import type {
  WorkerProjectionRequest,
  WorkerProjectionUpdate,
} from "./worker_protocol.js";

const PROJECTION_REQUEST_TAGS: ReadonlySet<string> = new Set([
  "Observe",
  "Unobserve",
  "ObserveDiagnostics",
  "Synchronize",
  "AcknowledgeProjection",
]);
const PROJECTION_UPDATE_TAGS: ReadonlySet<string> = new Set([
  "Lifecycle",
  "Interfaces",
  "Routes",
  "Links",
  "DiagnosticsReset",
  "DiagnosticsDelta",
]);
const PROJECTION_SYNCHRONIZATION_TAGS: ReadonlySet<string> = new Set([
  "Synchronized",
  "Busy",
  "Unavailable",
]);
const VIEW_TAGS: ReadonlySet<string> = new Set([
  "Lifecycle",
  "Interfaces",
  "Routes",
  "Links",
  "Diagnostics",
]);
const LIFECYCLE_TAGS: ReadonlySet<string> = new Set([
  "Starting",
  "Running",
  "Stopping",
  "Stopped",
  "Failed",
]);
const DIAGNOSTIC_TAGS: ReadonlySet<string> = new Set(
  Object.keys(DIAGNOSTIC_EVENT_KIND_CODES),
);

export function parseWorkerProjectionRequest(
  raw: unknown,
): WorkerProjectionRequest {
  const request = tagged<WorkerProjectionRequest>(
    raw,
    "WorkerProjectionRequest",
    PROJECTION_REQUEST_TAGS,
  );
  return match_into<WorkerProjectionRequest>().from(request, {
    Observe: (rawData) => {
      const data = record(rawData, "ObserveProjectionRequest");
      const view = parseView(field(data, "view"));
      if (view.tag === "Diagnostics") {
        throw new TypeError("diagnostic observation requires ObserveDiagnostics");
      }
      return Tag("Observe", { view });
    },
    Unobserve: (rawData) => {
      const data = record(rawData, "UnobserveProjectionRequest");
      const view = parseView(field(data, "view"));
      if (view.tag === "Diagnostics") {
        throw new TypeError("diagnostic observation requires ObserveDiagnostics");
      }
      return Tag("Unobserve", { view });
    },
    ObserveDiagnostics: (rawData) => {
      const data = record(rawData, "ObserveDiagnosticsRequest");
      return Tag("ObserveDiagnostics", {
        maximumEvents: nonNegativeSafeInteger(
          field(data, "maximumEvents"),
          "maximumEvents",
        ),
      });
    },
    Synchronize: (rawData) => {
      const data = record(rawData, "SynchronizeProjectionRequest");
      return Tag("Synchronize", {
        id: positiveSafeInteger(field(data, "id"), "id"),
        view: parseView(field(data, "view")),
      });
    },
    AcknowledgeProjection: (rawData) => {
      const data = record(rawData, "AcknowledgeProjectionRequest");
      return Tag("AcknowledgeProjection", {
        id: positiveSafeInteger(field(data, "id"), "id"),
      });
    },
  });
}

export function parseWorkerProjectionUpdate(raw: unknown): WorkerProjectionUpdate {
  const update = tagged<WorkerProjectionUpdate>(
    raw,
    "WorkerProjectionUpdate",
    PROJECTION_UPDATE_TAGS,
  );
  return match_into<WorkerProjectionUpdate>().from(update, {
    Lifecycle: (snapshot) => Tag(
      "Lifecycle",
      parseSnapshot(snapshot, "LifecycleProjection", parseLifecycle),
    ),
    Interfaces: (snapshot) => Tag(
      "Interfaces",
      parseSnapshot(snapshot, "InterfacesProjection", parseInterfaces),
    ),
    Routes: (snapshot) => Tag(
      "Routes",
      parseSnapshot(snapshot, "RoutesProjection", parseRoutes),
    ),
    Links: (snapshot) => Tag(
      "Links",
      parseSnapshot(snapshot, "LinksProjection", parseLinks),
    ),
    DiagnosticsReset: (snapshot) => Tag(
      "DiagnosticsReset",
      parseSnapshot(snapshot, "DiagnosticsProjection", parseDiagnostics),
    ),
    DiagnosticsDelta: (rawData) => {
      const data = record(rawData, "DiagnosticsProjectionDelta");
      return Tag("DiagnosticsDelta", {
        revision: asProjectionRevision(
          nonNegativeBigIntField(data, "revision"),
        ),
        dropped: nonNegativeSafeInteger(field(data, "dropped"), "dropped"),
        appended: parseDiagnostics(field(data, "appended")),
      });
    },
  });
}

export function parseProjectionSynchronization(
  view: PrnsView,
  raw: unknown,
): ProjectionSynchronization<unknown> {
  const outcome = tagged<ProjectionSynchronization<unknown>>(
    raw,
    "ProjectionSynchronization",
    PROJECTION_SYNCHRONIZATION_TAGS,
  );
  return match_into<ProjectionSynchronization<unknown>>().from(outcome, {
    Synchronized: (snapshot) => Tag(
      "Synchronized",
      parseSnapshotForView(view, snapshot),
    ),
    Busy: () => Tag("Busy"),
    Unavailable: (rawData) => {
      const data = record(rawData, "UnavailableProjectionSynchronization");
      const lifecycle = parseLifecycle(field(data, "lifecycle"));
      if (lifecycle.tag === "Running") {
        throw new TypeError("an unavailable projection cannot have a running lifecycle");
      }
      return Tag("Unavailable", { lifecycle });
    },
  });
}

function parseSnapshotForView(
  view: PrnsView,
  raw: unknown,
): PrnsProjectionSnapshot<unknown> {
  return match_into<PrnsProjectionSnapshot<unknown>>().from(view, {
    Lifecycle: () => parseSnapshot(raw, "LifecycleProjection", parseLifecycle),
    Interfaces: () => parseSnapshot(raw, "InterfacesProjection", parseInterfaces),
    Routes: () => parseSnapshot(raw, "RoutesProjection", parseRoutes),
    Links: () => parseSnapshot(raw, "LinksProjection", parseLinks),
    Diagnostics: () => parseSnapshot(
      raw,
      "DiagnosticsProjection",
      parseDiagnostics,
    ),
  });
}

function parseSnapshot<Value>(
  raw: unknown,
  name: string,
  parseValue: (raw: unknown) => Value,
): PrnsProjectionSnapshot<Value> {
  const snapshot = record(raw, name);
  return {
    revision: asProjectionRevision(
      nonNegativeBigIntField(snapshot, "revision"),
    ),
    value: parseValue(field(snapshot, "value")),
  };
}

function parseView(raw: unknown): PrnsView {
  const view = tagged<PrnsView>(raw, "PrnsView", VIEW_TAGS);
  return match_into<PrnsView>().from(view, {
    Lifecycle: () => prnsView("Lifecycle"),
    Interfaces: () => prnsView("Interfaces"),
    Routes: () => prnsView("Routes"),
    Links: () => prnsView("Links"),
    Diagnostics: (rawData) => {
      const data = record(rawData, "DiagnosticsView");
      return prnsView("Diagnostics", {
        maximumEvents: positiveSafeInteger(
          field(data, "maximumEvents"),
          "maximumEvents",
        ),
      });
    },
  });
}

function parseLifecycle(raw: unknown): LifecycleState {
  const lifecycle = tagged<LifecycleState>(
    raw,
    "LifecycleState",
    LIFECYCLE_TAGS,
  );
  return match_into<LifecycleState>().from(lifecycle, {
    Starting: () => Tag("Starting"),
    Running: () => Tag("Running"),
    Stopping: () => Tag("Stopping"),
    Stopped: (rawData) => {
      const data = record(rawData, "StoppedLifecycle");
      const reason = stringField(data, "reason");
      if (reason !== "Requested" && reason !== "BackendExited") {
        throw new TypeError("stopped lifecycle reason is invalid");
      }
      return Tag("Stopped", { reason });
    },
    Failed: (rawData) => {
      const data = record(rawData, "FailedLifecycle");
      const cause = stringField(data, "cause");
      if (cause === "EventBackpressureExceeded") {
        return Tag("Failed", {
          cause,
          limits: parseLimits(field(data, "limits")),
          rejectedEventBytes: nonNegativeSafeInteger(
            field(data, "rejectedEventBytes"),
            "rejectedEventBytes",
          ),
        });
      }
      if (cause === "BackendFailed" || cause === "ContractViolated") {
        return Tag("Failed", {
          cause,
          detail: stringField(data, "detail"),
        });
      }
      throw new TypeError("failed lifecycle cause is invalid");
    },
  });
}

function parseLimits(raw: unknown): PrnsLimits {
  const limits = record(raw, "PrnsLimits");
  return {
    pendingCommands: positiveSafeInteger(
      field(limits, "pendingCommands"),
      "pendingCommands",
    ),
    applicationEvents: positiveSafeInteger(
      field(limits, "applicationEvents"),
      "applicationEvents",
    ),
    retainedEventBytes: positiveSafeInteger(
      field(limits, "retainedEventBytes"),
      "retainedEventBytes",
    ),
    diagnostics: positiveSafeInteger(
      field(limits, "diagnostics"),
      "diagnostics",
    ),
  };
}

function parseInterfaces(raw: unknown): readonly InterfaceSnapshot[] {
  const values = array(raw, "interface projection");
  for (const value of values) {
    const entry = record(value, "InterfaceProjectionEntry");
    fixedBytes(field(entry, "interfaceId"), INTERFACE_ID_LENGTH, "interfaceId");
    const health = field(entry, "health");
    if (!isInterfaceHealth(health)) {
      throw new TypeError("interface health is invalid");
    }
    optionalString(entry, "name");
    const kind = optional(entry, "kind");
    if (kind !== undefined && !isInterfaceKind(kind)) {
      throw new TypeError("interface kind is invalid");
    }
    nonNegativeBigInt(entry, "rxBytes");
    nonNegativeBigInt(entry, "txBytes");
    optionalNonNegativeNumber(entry, "rxBps");
    optionalNonNegativeNumber(entry, "txBps");
    nonNegativeSafeInteger(field(entry, "routeCount"), "routeCount");
    nonNegativeSafeInteger(field(entry, "linkCount"), "linkCount");
    nonNegativeSafeInteger(
      field(entry, "transportedLinkCount"),
      "transportedLinkCount",
    );
  }
  return values as readonly InterfaceSnapshot[];
}

function parseRoutes(raw: unknown): readonly RouteSnapshot[] {
  const values = array(raw, "route projection");
  for (const value of values) {
    const entry = record(value, "RouteProjectionEntry");
    fixedBytes(
      field(entry, "destination"),
      DESTINATION_HASH_LENGTH,
      "destination",
    );
    nonNegativeSafeInteger(field(entry, "hops"), "hops");
    const viaIdentity = optional(entry, "viaIdentity");
    if (viaIdentity !== undefined) {
      fixedBytes(viaIdentity, IDENTITY_HASH_LENGTH, "viaIdentity");
    }
    fixedBytes(
      field(entry, "interfaceId"),
      INTERFACE_ID_LENGTH,
      "interfaceId",
    );
    nonNegativeSafeInteger(
      field(entry, "learnedAtMillis"),
      "learnedAtMillis",
    );
    nonNegativeSafeInteger(
      field(entry, "lastRouteActivityAtMillis"),
      "lastRouteActivityAtMillis",
    );
    nonNegativeSafeInteger(
      field(entry, "expiresAtMillis"),
      "expiresAtMillis",
    );
  }
  return values as readonly RouteSnapshot[];
}

function parseLinks(raw: unknown): readonly ActiveLinkSnapshot[] {
  const values = array(raw, "link projection");
  for (const value of values) {
    const entry = record(value, "LinkProjectionEntry");
    fixedBytes(field(entry, "linkId"), LINK_ID_LENGTH, "linkId");
    nonNegativeSafeInteger(field(entry, "rttMillis"), "rttMillis");
    const peerIdentity = optional(entry, "peerIdentity");
    if (peerIdentity !== undefined) {
      fixedBytes(peerIdentity, IDENTITY_HASH_LENGTH, "peerIdentity");
    }
  }
  return values as readonly ActiveLinkSnapshot[];
}

function parseDiagnostics(raw: unknown): readonly PrnsDiagnosticEvent[] {
  const values = array(raw, "diagnostic projection");
  for (const value of values) {
    parseWorkerDiagnosticEvent(value);
  }
  return values as readonly PrnsDiagnosticEvent[];
}

export function parseWorkerDiagnosticEvent(raw: unknown): PrnsDiagnosticEvent {
  const event = tagged<PrnsDiagnosticEvent>(
    raw,
    "WorkerDiagnosticEvent",
    DIAGNOSTIC_TAGS,
  );
  record(event.data, "WorkerDiagnosticEventData");
  return event;
}

function tagged<Value>(
  raw: unknown,
  name: string,
  allowedTags: ReadonlySet<string>,
): Value {
  const envelope = record(raw, name);
  const tag = stringField(envelope, "tag");
  if (!allowedTags.has(tag)) {
    throw new TypeError(`${name} contains an unknown tag`);
  }
  return raw as Value;
}

function array(raw: unknown, name: string): readonly unknown[] {
  if (!Array.isArray(raw)) {
    throw new TypeError(`${name} must be an array`);
  }
  return raw;
}

function fixedBytes(
  raw: unknown,
  length: number,
  name: string,
): asserts raw is Uint8Array {
  if (!(raw instanceof Uint8Array) || raw.length !== length) {
    throw new TypeError(`${name} must contain exactly ${length} bytes`);
  }
}

function positiveSafeInteger(raw: unknown, name: string): number {
  if (typeof raw !== "number" || !Number.isSafeInteger(raw) || raw <= 0) {
    throw new TypeError(`${name} must be a positive safe integer`);
  }
  return raw;
}

function nonNegativeSafeInteger(raw: unknown, name: string): number {
  if (typeof raw !== "number" || !Number.isSafeInteger(raw) || raw < 0) {
    throw new TypeError(`${name} must be a non-negative safe integer`);
  }
  return raw;
}

function nonNegativeBigInt(object: Record<string, unknown>, name: string): bigint {
  return nonNegativeBigIntField(object, name);
}

function optional(
  object: Record<string, unknown>,
  name: string,
): unknown | undefined {
  return name in object ? object[name] : undefined;
}

function optionalString(object: Record<string, unknown>, name: string): void {
  const value = optional(object, name);
  if (value !== undefined && typeof value !== "string") {
    throw new TypeError(`${name} must be a string`);
  }
}

function optionalNonNegativeNumber(
  object: Record<string, unknown>,
  name: string,
): void {
  const value = optional(object, name);
  if (
    value !== undefined &&
    (typeof value !== "number" || !Number.isFinite(value) || value < 0)
  ) {
    throw new TypeError(`${name} must be a non-negative finite number`);
  }
}
