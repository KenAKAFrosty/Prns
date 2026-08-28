import {
  APPLICATION_EVENT_KIND_CODES,
  DIAGNOSTIC_EVENT_KIND_CODES,
  EVENT_FIELD_CODES,
  HOST_SCHEMA_VERSION,
} from "./contract.generated.js";

const EVENT_BATCH_MAGIC = 0x454e5250;
const EVENT_BATCH_FORMAT_VERSION = 1;
const EVENT_BATCH_HEADER_BYTES = 16;
const EVENT_RECORD_HEADER_BYTES = 8;
const EVENT_FIELD_HEADER_BYTES = 8;
const textDecoder = new TextDecoder("utf-8", { fatal: true });

export type EventProjectionValue = Uint8Array | string | bigint;

export type EventProjection = {
  readonly kind: number;
  readonly fields: ReadonlyMap<number, EventProjectionValue>;
};

export type EventBatchProjectionSummary = {
  readonly applicationEvents: number;
  readonly diagnostics: number;
  readonly retainedEventBytes: number;
};

export type EventBatchProjectionFailure =
  | "Truncated"
  | "InvalidMagic"
  | "UnsupportedFormat"
  | "SchemaMismatch"
  | "InvalidReservedValue"
  | "InvalidKind"
  | "InvalidFieldId"
  | "DuplicateField"
  | "UnknownWireType"
  | "InvalidWireLength"
  | "InvalidText"
  | "TrailingBytes";

export class EventBatchProjectionError extends Error {
  readonly code: EventBatchProjectionFailure;
  readonly offset: number;

  constructor(
    code: EventBatchProjectionFailure,
    offset: number,
    message: string,
  ) {
    super(message);
    this.name = "EventBatchProjectionError";
    this.code = code;
    this.offset = offset;
  }
}

export function decodeEventBatchProjection(
  bytes: Uint8Array,
  acceptedKinds?: ReadonlySet<number>,
): readonly EventProjection[] {
  const records: EventProjection[] = [];
  visitEventBatchProjection(bytes, acceptedKinds, (kind) => {
    const fields = new Map<number, EventProjectionValue>();
    return {
      field: (id, wireType, offset, end, view) => {
        fields.set(id, decodeValue(bytes, view, offset, end, wireType));
      },
      finish: () => records.push(Object.freeze({ kind, fields })),
    };
  });
  return Object.freeze(records);
}

export function summarizeEventBatchProjection(
  bytes: Uint8Array,
): EventBatchProjectionSummary {
  let applicationEvents = 0;
  let diagnostics = 0;
  let retainedEventBytes = 0;
  visitEventBatchProjection(bytes, undefined, (kind, start) => {
    if (APPLICATION_EVENT_KINDS.has(kind)) {
      applicationEvents += 1;
    } else if (DIAGNOSTIC_EVENT_KINDS.has(kind)) {
      diagnostics += 1;
    } else {
      throw projectionError(
        "InvalidKind",
        start + 4,
        `event projection kind ${kind} is unknown`,
      );
    }
    return {
      field: (id, _wireType, offset, end) => {
        if (retainsField(kind, id)) {
          retainedEventBytes = checkedTotal(retainedEventBytes, end - offset, offset);
        }
      },
      finish: () => undefined,
    };
  });
  return Object.freeze({
    applicationEvents,
    diagnostics,
    retainedEventBytes,
  });
}

export function retainApplicationEventBatchProjection(
  bytes: Uint8Array,
): Uint8Array {
  const retainedRecords: Array<{ readonly start: number; readonly end: number }> = [];
  visitEventBatchProjection(bytes, undefined, (kind, start, end) => ({
    field: () => undefined,
    finish: () => {
      if (APPLICATION_EVENT_KINDS.has(kind)) {
        retainedRecords.push({ start, end });
      }
    },
  }));
  const byteLength = retainedRecords.reduce(
    (total, record) => checkedTotal(total, record.end - record.start, record.start),
    EVENT_BATCH_HEADER_BYTES,
  );
  const retained = new Uint8Array(byteLength);
  retained.set(bytes.subarray(0, EVENT_BATCH_HEADER_BYTES));
  new DataView(retained.buffer).setUint32(12, retainedRecords.length, true);
  let offset = EVENT_BATCH_HEADER_BYTES;
  for (const record of retainedRecords) {
    const encoded = bytes.subarray(record.start, record.end);
    retained.set(encoded, offset);
    offset += encoded.byteLength;
  }
  return retained;
}

type EventProjectionRecordVisitor = {
  readonly field: (
    id: number,
    wireType: number,
    offset: number,
    end: number,
    view: DataView,
  ) => void;
  readonly finish: () => void;
};

function visitEventBatchProjection(
  bytes: Uint8Array,
  acceptedKinds: ReadonlySet<number> | undefined,
  visitRecord: (
    kind: number,
    start: number,
    end: number,
  ) => EventProjectionRecordVisitor,
): void {
  if (bytes.byteLength < EVENT_BATCH_HEADER_BYTES) {
    throw projectionError("Truncated", bytes.byteLength, "event batch header is truncated");
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (view.getUint32(0, true) !== EVENT_BATCH_MAGIC) {
    throw projectionError("InvalidMagic", 0, "event batch magic does not match");
  }
  if (view.getUint16(4, true) !== EVENT_BATCH_FORMAT_VERSION) {
    throw projectionError(
      "UnsupportedFormat",
      4,
      "event batch format version is unsupported",
    );
  }
  if (view.getUint16(6, true) !== 0) {
    throw projectionError(
      "InvalidReservedValue",
      6,
      "event batch reserved header bits are nonzero",
    );
  }
  if (view.getUint32(8, true) !== HOST_SCHEMA_VERSION) {
    throw projectionError(
      "SchemaMismatch",
      8,
      "event batch host schema version does not match",
    );
  }
  const recordCount = view.getUint32(12, true);
  let offset = EVENT_BATCH_HEADER_BYTES;
  for (let recordIndex = 0; recordIndex < recordCount; recordIndex += 1) {
    const recordStart = offset;
    requireAvailable(bytes, offset, EVENT_RECORD_HEADER_BYTES);
    const bodyBytes = view.getUint32(offset, true);
    const kind = view.getUint16(offset + 4, true);
    const fieldCount = view.getUint16(offset + 6, true);
    if (kind === 0) {
      throw projectionError("InvalidKind", offset + 4, "event projection kind is zero");
    }
    offset += EVENT_RECORD_HEADER_BYTES;
    const recordEnd = checkedEnd(bytes, offset, bodyBytes);
    if (acceptedKinds !== undefined && !acceptedKinds.has(kind)) {
      offset = recordEnd;
      continue;
    }
    const visitor = visitRecord(kind, recordStart, recordEnd);
    const fieldIds = new Set<number>();
    for (let fieldIndex = 0; fieldIndex < fieldCount; fieldIndex += 1) {
      requireAvailable(bytes, offset, EVENT_FIELD_HEADER_BYTES);
      const id = view.getUint16(offset, true);
      const wireType = view.getUint8(offset + 2);
      const reserved = view.getUint8(offset + 3);
      const valueBytes = view.getUint32(offset + 4, true);
      if (id === 0) {
        throw projectionError("InvalidFieldId", offset, "event projection field id is zero");
      }
      if (reserved !== 0) {
        throw projectionError(
          "InvalidReservedValue",
          offset + 3,
          "event projection field reserved bits are nonzero",
        );
      }
      if (fieldIds.has(id)) {
        throw projectionError(
          "DuplicateField",
          offset,
          `event projection field ${id} appears more than once`,
        );
      }
      fieldIds.add(id);
      offset += EVENT_FIELD_HEADER_BYTES;
      const valueEnd = checkedEnd(bytes, offset, valueBytes);
      if (valueEnd > recordEnd) {
        throw projectionError(
          "Truncated",
          offset,
          "event projection field exceeds its record boundary",
        );
      }
      visitor.field(id, wireType, offset, valueEnd, view);
      offset = valueEnd;
    }
    if (offset !== recordEnd) {
      throw projectionError(
        "TrailingBytes",
        offset,
        "event projection record has trailing bytes",
      );
    }
    visitor.finish();
  }
  if (offset !== bytes.byteLength) {
    throw projectionError(
      "TrailingBytes",
      offset,
      "event batch has trailing bytes",
    );
  }
}

const APPLICATION_EVENT_KINDS: ReadonlySet<number> = new Set(
  Object.values(APPLICATION_EVENT_KIND_CODES),
);
const DIAGNOSTIC_EVENT_KINDS: ReadonlySet<number> = new Set(
  Object.values(DIAGNOSTIC_EVENT_KIND_CODES),
);

function retainsField(kind: number, field: number): boolean {
  switch (kind) {
    case APPLICATION_EVENT_KIND_CODES.SingleDelivery:
    case APPLICATION_EVENT_KIND_CODES.LinkDelivery:
      return field === EVENT_FIELD_CODES.Plaintext;
    case APPLICATION_EVENT_KIND_CODES.Request:
    case APPLICATION_EVENT_KIND_CODES.Response:
    case APPLICATION_EVENT_KIND_CODES.ResponseSegment:
    case APPLICATION_EVENT_KIND_CODES.ChannelMessage:
      return field === EVENT_FIELD_CODES.Data;
    case APPLICATION_EVENT_KIND_CODES.ResourceAvailable:
    case APPLICATION_EVENT_KIND_CODES.ResourceSegment:
      return field === EVENT_FIELD_CODES.Data || field === EVENT_FIELD_CODES.Metadata;
    case APPLICATION_EVENT_KIND_CODES.ResourceNeedsDecompression:
      return field === EVENT_FIELD_CODES.Stream;
    default:
      return false;
  }
}

function checkedTotal(total: number, added: number, offset: number): number {
  const sum = total + added;
  if (!Number.isSafeInteger(sum)) {
    throw projectionError(
      "InvalidWireLength",
      offset,
      "event batch retained byte total exceeds the JavaScript safe-integer limit",
    );
  }
  return sum;
}

function decodeValue(
  bytes: Uint8Array,
  view: DataView,
  offset: number,
  end: number,
  wireType: number,
): EventProjectionValue {
  if (wireType === 1) {
    return bytes.subarray(offset, end);
  }
  if (wireType === 2) {
    try {
      return textDecoder.decode(bytes.subarray(offset, end));
    } catch {
      throw projectionError("InvalidText", offset, "event projection text is not UTF-8");
    }
  }
  if (wireType === 3) {
    if (end - offset !== 8) {
      throw projectionError("InvalidWireLength", offset, "u64 event field is not 8 bytes");
    }
    return view.getBigUint64(offset, true);
  }
  if (wireType === 4) {
    if (end - offset !== 16) {
      throw projectionError("InvalidWireLength", offset, "u128 event field is not 16 bytes");
    }
    return view.getBigUint64(offset, true) |
      (view.getBigUint64(offset + 8, true) << 64n);
  }
  throw projectionError(
    "UnknownWireType",
    offset,
    `event projection wire type ${wireType} is unknown`,
  );
}

function requireAvailable(bytes: Uint8Array, offset: number, length: number): void {
  checkedEnd(bytes, offset, length);
}

function checkedEnd(bytes: Uint8Array, offset: number, length: number): number {
  const end = offset + length;
  if (!Number.isSafeInteger(end) || end > bytes.byteLength) {
    throw projectionError("Truncated", offset, "event batch is truncated");
  }
  return end;
}

function projectionError(
  code: EventBatchProjectionFailure,
  offset: number,
  message: string,
): EventBatchProjectionError {
  return new EventBatchProjectionError(code, offset, message);
}
