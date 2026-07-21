import contract from "../bridge-contract.json" with { type: "json" };

export const BRIDGE_SCHEMA = contract.schema;
export const RESPONSE_LIMITS = Object.freeze({ ...contract.response_limits });

const phaseDefinitions = new Map(contract.phases.map((phase) => [phase.wire, phase]));
const operationDefinitions = new Map(
  contract.operations.map((operation) => [operation.wire, operation]),
);
const errors = new Set(contract.errors);
const eventFields = new Set(contract.event_fields);

validateContractDefinition();

export function validateBridgeEvent(event) {
  if (!event || typeof event !== "object" || Array.isArray(event)) {
    throw new TypeError("A bridge event must be an object.");
  }
  for (const field of Object.keys(event)) {
    if (!eventFields.has(field)) {
      throw new TypeError(`Bridge event field ${field} is not in schema ${BRIDGE_SCHEMA}.`);
    }
  }
  if (event.schema !== BRIDGE_SCHEMA) {
    throw new TypeError(`Bridge event schema ${event.schema} is unsupported.`);
  }
  const phase = phaseDefinitions.get(event.phase);
  if (!phase) {
    throw new TypeError(`Bridge phase ${event.phase} is not in schema ${BRIDGE_SCHEMA}.`);
  }
  if (event.code !== undefined && !errors.has(event.code)) {
    throw new TypeError(`Bridge error ${event.code} is not in schema ${BRIDGE_SCHEMA}.`);
  }
  for (const field of ["current", "total", "partIndex", "partCount", "bytes"]) {
    if (event[field] !== undefined && (!Number.isSafeInteger(event[field]) || event[field] < 0)) {
      throw new TypeError(`Bridge event field ${field} must be a non-negative safe integer.`);
    }
  }
  for (const field of ["message", "part", "detectedChip"]) {
    if (event[field] !== undefined && typeof event[field] !== "string") {
      throw new TypeError(`Bridge event field ${field} must be a string.`);
    }
  }
  validateErrorPolicy(event, phase);
  validateProgress(event, phase);
  validatePartProgress(event, phase);
  validateExclusiveField(event, phase, "detectedChip", phase.detected_chip);
  validateExclusiveField(event, phase, "bytes", phase.bytes);
  return event;
}

export class BridgeEventSequence {
  #definition;
  #phase = null;
  #terminal = false;
  #current = null;
  #total = null;

  constructor(operation) {
    this.#definition = operationDefinitions.get(operation);
    if (!this.#definition) {
      throw new TypeError(`Bridge operation ${operation} is not in schema ${BRIDGE_SCHEMA}.`);
    }
  }

  get terminal() {
    return this.#terminal;
  }

  accept(event) {
    const value = validateBridgeEvent(event);
    if (this.#terminal) {
      throw new TypeError("A bridge operation cannot emit after its terminal event.");
    }

    const allowed = this.#phase === null
      ? this.#definition.initial
      : phaseDefinitions.get(this.#phase).next;
    if (!allowed.includes(value.phase)) {
      const previous = this.#phase ?? `${this.#definition.wire} start`;
      throw new TypeError(`Bridge transition ${previous} -> ${value.phase} is not permitted.`);
    }

    if (value.current !== undefined) {
      if (this.#total !== null && value.total !== this.#total) {
        throw new TypeError("Bridge progress total changed during one operation.");
      }
      if (this.#current !== null && value.current < this.#current) {
        throw new TypeError("Bridge progress moved backwards during one operation.");
      }
      this.#current = value.current;
      this.#total = value.total;
    }

    this.#phase = value.phase;
    this.#terminal = phaseDefinitions.get(value.phase).terminal;
    return value;
  }
}

function validateErrorPolicy(event, phase) {
  if (phase.error_policy === "forbidden" && event.code !== undefined) {
    throw new TypeError(`Bridge phase ${event.phase} cannot carry an error code.`);
  }
  if (phase.error_policy === "failure" && (event.code === undefined || event.code === "cancelled")) {
    throw new TypeError("Bridge failed events require a non-cancellation error code.");
  }
  if (phase.error_policy === "cancelled" && event.code !== "cancelled") {
    throw new TypeError("Bridge cancelled events require the cancelled error code.");
  }
  if (
    phase.error_policy !== "forbidden"
    && (typeof event.message !== "string" || event.message.trim().length === 0)
  ) {
    throw new TypeError(`Bridge ${event.phase} events require a recovery message.`);
  }
}

function validateProgress(event, phase) {
  const hasCurrent = event.current !== undefined;
  const hasTotal = event.total !== undefined;
  if (hasCurrent !== hasTotal) {
    throw new TypeError("Bridge progress requires both current and total bytes.");
  }
  if (phase.progress === "forbidden" && hasCurrent) {
    throw new TypeError(`Bridge phase ${event.phase} cannot carry byte progress.`);
  }
  if (phase.progress !== "forbidden" && !hasCurrent) {
    throw new TypeError(`Bridge phase ${event.phase} requires byte progress.`);
  }
  if (hasCurrent && event.current > event.total) {
    throw new TypeError("Bridge progress current bytes exceed total bytes.");
  }
  if (hasCurrent && event.total < contract.minimum_progress_total) {
    throw new TypeError(
      `Bridge progress total must be at least ${contract.minimum_progress_total} byte.`,
    );
  }
  if (phase.progress === "complete" && event.current !== event.total) {
    throw new TypeError(`Bridge phase ${event.phase} requires complete byte progress.`);
  }
}

function validatePartProgress(event, phase) {
  const fields = [event.part, event.partIndex, event.partCount];
  const present = fields.filter((value) => value !== undefined).length;
  if (present !== 0 && present !== fields.length) {
    throw new TypeError("Bridge part progress requires part, partIndex, and partCount together.");
  }
  if (phase.parts === "forbidden" && present !== 0) {
    throw new TypeError(`Bridge phase ${event.phase} cannot carry part progress.`);
  }
  if (phase.parts === "required" && present !== fields.length) {
    throw new TypeError(`Bridge phase ${event.phase} requires part progress.`);
  }
  if (present === fields.length) {
    if (event.part.length === 0 || event.partCount === 0 || event.partIndex >= event.partCount) {
      throw new TypeError("Bridge part progress is outside its declared part count.");
    }
  }
}

function validateExclusiveField(event, phase, field, required) {
  const present = event[field] !== undefined;
  if (required && !present) {
    throw new TypeError(`Bridge phase ${event.phase} requires ${field}.`);
  }
  if (!required && present) {
    throw new TypeError(`Bridge phase ${event.phase} cannot carry ${field}.`);
  }
  if (field === "detectedChip" && present && event[field].trim().length === 0) {
    throw new TypeError("Bridge detectedChip cannot be empty.");
  }
}

function validateContractDefinition() {
  if (BRIDGE_SCHEMA !== 1) {
    throw new TypeError(`Bundled bridge schema ${BRIDGE_SCHEMA} is unsupported.`);
  }
  if (contract.minimum_progress_total !== 1) {
    throw new TypeError("Bundled bridge minimum progress total is unsupported.");
  }
  const expectedResponseLimits = {
    channel_bytes: 64 * 1024,
    manifest_bytes: 512 * 1024,
    signature_bytes: 64 * 1024,
    artifact_bytes: 64 * 1024 * 1024,
  };
  for (const [name, expected] of Object.entries(expectedResponseLimits)) {
    if (contract.response_limits?.[name] !== expected) {
      throw new TypeError(`Bundled bridge response limit ${name} is unsupported.`);
    }
  }
  assertUniqueDefinition(phaseDefinitions, contract.phases, "phase");
  assertUniqueDefinition(operationDefinitions, contract.operations, "operation");
  assertUniqueValues(errors, contract.errors, "error");
  assertUniqueValues(eventFields, contract.event_fields, "event field");

  const errorPolicies = new Set(["forbidden", "failure", "cancelled"]);
  const progressPolicies = new Set(["forbidden", "required", "complete"]);
  const partPolicies = new Set(["forbidden", "optional", "required"]);
  for (const phase of contract.phases) {
    if (
      !errorPolicies.has(phase.error_policy)
      || !progressPolicies.has(phase.progress)
      || !partPolicies.has(phase.parts)
      || typeof phase.detected_chip !== "boolean"
      || typeof phase.bytes !== "boolean"
      || !Array.isArray(phase.next)
      || phase.next.some((next) => !phaseDefinitions.has(next))
      || new Set(phase.next).size !== phase.next.length
      || (phase.terminal && phase.next.length !== 0)
    ) {
      throw new TypeError(`Bundled bridge phase ${phase.wire} is invalid.`);
    }
  }
  for (const operation of contract.operations) {
    if (
      !Array.isArray(operation.initial)
      || operation.initial.length === 0
      || operation.initial.some((phase) => !phaseDefinitions.has(phase))
      || new Set(operation.initial).size !== operation.initial.length
    ) {
      throw new TypeError(`Bundled bridge operation ${operation.wire} is invalid.`);
    }
  }
}

function assertUniqueDefinition(index, definitions, kind) {
  if (index.size !== definitions.length || definitions.some(({ wire }) => typeof wire !== "string")) {
    throw new TypeError(`Bundled bridge ${kind} definitions are not unique.`);
  }
}

function assertUniqueValues(index, values, kind) {
  if (index.size !== values.length || values.some((value) => typeof value !== "string")) {
    throw new TypeError(`Bundled bridge ${kind} values are not unique.`);
  }
}

export const testingContract = contract;
