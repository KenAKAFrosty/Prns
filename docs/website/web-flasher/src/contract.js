import contract from "../bridge-contract.json" with { type: "json" };

export const BRIDGE_SCHEMA = contract.schema;

const phases = new Set(contract.phases.map((phase) => phase.wire));
const errors = new Set(contract.errors);
const eventFields = new Set(contract.event_fields);

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
  if (!phases.has(event.phase)) {
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
  return event;
}

export const testingContract = contract;
