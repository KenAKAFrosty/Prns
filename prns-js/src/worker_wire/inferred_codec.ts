import { Tag } from "../casework.js";
import type { Tag as Tagged } from "../casework.js";

export type WireSchema =
  | Tagged<"Empty">
  | Tagged<"Null">
  | Tagged<"Undefined">
  | Tagged<"Number">
  | Tagged<"Boolean">
  | Tagged<"StringUtf8">
  | Tagged<"StringDictionary", { readonly codeBytes: 1 | 2 | 4 }>
  | Tagged<"Date">
  | Tagged<"Bytes">
  | Tagged<"List", { readonly item: WireSchema }>
  | Tagged<
      "Record",
      {
        readonly prototype: "Object" | "Null";
        readonly fields: readonly WireField[];
      }
    >
  | Tagged<"Nullable", { readonly value: WireSchema }>
  | Tagged<"MaybeUndefined", { readonly value: WireSchema }>
  | Tagged<
      "Union",
      {
        readonly codeBytes: 1 | 2 | 4;
        readonly variants: readonly WireSchema[];
      }
    >
  | Tagged<"TaggedUnion", { readonly variants: readonly WireTaggedVariant[] }>;

export type WireField = {
  readonly name: string;
  readonly optional: boolean;
  readonly value: WireSchema;
};

export type WireTaggedVariant = {
  readonly tag: string;
  readonly value: WireSchema;
};

export type PackedValue = {
  readonly schema: WireSchema;
  readonly fingerprint: string;
  readonly buffer: ArrayBuffer;
};

export type InferredPackingOutcome =
  | Tagged<"Packed", PackedValue>
  | Tagged<"UseStructuredClone", { readonly reason: PackingDeclineReason }>;

export type PackingDeclineReason =
  | "UnsupportedValue"
  | "ObjectIdentity"
  | "AccessorProperty"
  | "SymbolProperty"
  | "SparseArray"
  | "ArrayProperty"
  | "MaximumDepthExceeded";

const MAXIMUM_INFERENCE_DEPTH = 32;
const MAXIMUM_ESTIMATION_VALUES = 1_024;
const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder("utf-8", { fatal: true });
const encodingGenerations = new WeakMap<object, number>();
const recordFieldNames = new WeakMap<object, ReadonlySet<string>>();
let nextEncodingGeneration = 1;

assertLittleEndian();

export function inferPackedValue(value: unknown): InferredPackingOutcome {
  const inferred = inferWireSchema(value);
  if (inferred.tag === "UseStructuredClone") {
    return inferred;
  }
  return Tag("Packed", packWireValue(inferred.data.schema, value));
}

export function inferWireSchema(
  value: unknown,
): Tagged<"Inferred", { readonly schema: WireSchema }> | Extract<InferredPackingOutcome, Tagged<"UseStructuredClone", unknown>> {
  const supported = inspectValue(value, new WeakSet(), 0);
  if (supported.tag === "Declined") {
    return Tag("UseStructuredClone", { reason: supported.data });
  }
  return Tag("Inferred", { schema: inferColumn([value]) });
}

export function packWireValue(schema: WireSchema, value: unknown): PackedValue {
  const writer = new BinaryWriter();
  const generation = nextEncodingGeneration;
  nextEncodingGeneration = generation === Number.MAX_SAFE_INTEGER ? 1 : generation + 1;
  encodeColumn(schema, [value], writer, { generation }, 0);
  return {
    schema,
    fingerprint: JSON.stringify(schema),
    buffer: writer.finish(),
  };
}

export function packingSupport(
  value: unknown,
): Tagged<"Supported"> | Tagged<"Declined", PackingDeclineReason> {
  return inspectValue(value, new WeakSet(), 0);
}

export function estimateWireBytes(value: unknown): number {
  return estimateValueBytes(value, {
    seen: new WeakSet(),
    remainingValues: MAXIMUM_ESTIMATION_VALUES,
  }, 0);
}

type EstimationState = {
  readonly seen: WeakSet<object>;
  remainingValues: number;
};

function estimateValueBytes(
  value: unknown,
  state: EstimationState,
  depth: number,
): number {
  if (depth > MAXIMUM_INFERENCE_DEPTH || state.remainingValues === 0) {
    return Number.MAX_SAFE_INTEGER;
  }
  state.remainingValues -= 1;
  if (value === null || value === undefined) {
    return 1;
  }
  if (typeof value === "number") {
    return 8;
  }
  if (typeof value === "boolean") {
    return 1;
  }
  if (typeof value === "string") {
    return 4 + value.length * 3;
  }
  if (value instanceof Date) {
    return 8;
  }
  if (value instanceof Uint8Array) {
    return 4 + value.byteLength;
  }
  if (typeof Blob !== "undefined" && value instanceof Blob) {
    return value.size;
  }
  if (Array.isArray(value)) {
    if (state.seen.has(value)) {
      return 16;
    }
    state.seen.add(value);
    let total = 4;
    for (let index = 0; index < value.length; index += 1) {
      const descriptor = Object.getOwnPropertyDescriptor(value, index);
      if (descriptor === undefined || !("value" in descriptor)) {
        return boundedAdd(total, 16);
      }
      total = boundedAdd(
        total,
        estimateValueBytes(descriptor.value, state, depth + 1),
      );
      if (total === Number.MAX_SAFE_INTEGER) {
        return total;
      }
    }
    return total;
  }
  if (typeof value === "object") {
    if (state.seen.has(value)) {
      return 16;
    }
    state.seen.add(value);
    let total = 4;
    for (const key of Object.keys(value)) {
      const descriptor = Object.getOwnPropertyDescriptor(value, key);
      if (descriptor === undefined || !("value" in descriptor)) {
        return boundedAdd(total, 16);
      }
      total = boundedAdd(
        total,
        key.length * 3 + estimateValueBytes(descriptor.value, state, depth + 1),
      );
      if (total === Number.MAX_SAFE_INTEGER) {
        return total;
      }
    }
    return total;
  }
  return 16;
}

export function decodePackedValue(
  schema: WireSchema,
  buffer: ArrayBuffer,
): unknown {
  const reader = new BinaryReader(buffer);
  const values = decodeColumn(schema, reader);
  reader.requireFinished();
  if (values.length !== 1) {
    throw new TypeError("packed root did not decode to one value");
  }
  return values[0];
}

type InspectionOutcome =
  | Tagged<"Supported">
  | Tagged<"Declined", PackingDeclineReason>;

function inspectValue(
  value: unknown,
  seen: WeakSet<object>,
  depth: number,
): InspectionOutcome {
  if (depth > MAXIMUM_INFERENCE_DEPTH) {
    return Tag("Declined", "MaximumDepthExceeded");
  }
  if (
    value === null ||
    value === undefined ||
    typeof value === "number" ||
    typeof value === "boolean" ||
    typeof value === "string"
  ) {
    return Tag("Supported");
  }
  if (typeof value !== "object") {
    return Tag("Declined", "UnsupportedValue");
  }
  if (seen.has(value)) {
    return Tag("Declined", "ObjectIdentity");
  }
  seen.add(value);
  if (value instanceof Date || value instanceof Uint8Array) {
    return Tag("Supported");
  }
  if (Array.isArray(value)) {
    const ownNames = Object.keys(value);
    for (const symbol of Object.getOwnPropertySymbols(value)) {
      if (Object.getOwnPropertyDescriptor(value, symbol)?.enumerable === true) {
        return Tag("Declined", "SymbolProperty");
      }
    }
    for (let index = 0; index < value.length; index += 1) {
      const descriptor = Object.getOwnPropertyDescriptor(value, index);
      if (descriptor === undefined) {
        return Tag("Declined", "SparseArray");
      }
      if (!("value" in descriptor)) {
        return Tag("Declined", "AccessorProperty");
      }
    }
    if (ownNames.some((name) => !isArrayIndex(name, value.length))) {
      return Tag("Declined", "ArrayProperty");
    }
    for (let index = 0; index < value.length; index += 1) {
      const descriptor = Object.getOwnPropertyDescriptor(value, index) as PropertyDescriptor & {
        readonly value: unknown;
      };
      const inspected = inspectValue(descriptor.value, seen, depth + 1);
      if (inspected.tag === "Declined") {
        return inspected;
      }
    }
    return Tag("Supported");
  }
  const prototype = Object.getPrototypeOf(value);
  if (prototype !== Object.prototype && prototype !== null) {
    return Tag("Declined", "UnsupportedValue");
  }
  for (const symbol of Object.getOwnPropertySymbols(value)) {
    if (Object.getOwnPropertyDescriptor(value, symbol)?.enumerable === true) {
      return Tag("Declined", "SymbolProperty");
    }
  }
  for (const name of Object.keys(value)) {
    const descriptor = Object.getOwnPropertyDescriptor(value, name);
    if (descriptor === undefined || !("value" in descriptor)) {
      return Tag("Declined", "AccessorProperty");
    }
    const inspected = inspectValue(descriptor.value, seen, depth + 1);
    if (inspected.tag === "Declined") {
      return inspected;
    }
  }
  return Tag("Supported");
}

function isArrayIndex(name: string, length: number): boolean {
  if (name === "") {
    return false;
  }
  const value = Number(name);
  return Number.isSafeInteger(value) && value >= 0 && value < length && String(value) === name;
}

function inferColumn(values: readonly unknown[]): WireSchema {
  if (values.length === 0) {
    return Tag("Empty");
  }
  const nonNull = values.filter((value) => value !== null);
  if (nonNull.length === 0) {
    return Tag("Null");
  }
  if (nonNull.length !== values.length) {
    return Tag("Nullable", { value: inferColumn(nonNull) });
  }
  const defined = values.filter((value) => value !== undefined);
  if (defined.length === 0) {
    return Tag("Undefined");
  }
  if (defined.length !== values.length) {
    return Tag("MaybeUndefined", { value: inferColumn(defined) });
  }
  const groups = groupValues(values);
  if (groups.length > 1) {
    return Tag("Union", {
      codeBytes: codeWidth(groups.length),
      variants: groups.map((group) => inferColumn(group.values)),
    });
  }
  const group = groups[0];
  if (group === undefined) {
    return Tag("Empty");
  }
  if (group.key === "Number") {
    return Tag("Number");
  }
  if (group.key === "Boolean") {
    return Tag("Boolean");
  }
  if (group.key === "String") {
    return inferStringSchema(values as readonly string[]);
  }
  if (group.key === "Date") {
    return Tag("Date");
  }
  if (group.key === "Bytes") {
    return Tag("Bytes");
  }
  if (group.key === "List") {
    const flattened: unknown[] = [];
    for (const value of values as readonly unknown[][]) {
      for (const item of value) {
        flattened.push(item);
      }
    }
    return Tag("List", { item: inferColumn(flattened) });
  }
  if (group.key === "TaggedUnion") {
    const byTag = new Map<string, unknown[]>();
    for (const value of values as readonly { readonly tag: string; readonly data: unknown }[]) {
      const tagged = byTag.get(value.tag);
      if (tagged === undefined) {
        byTag.set(value.tag, [value.data]);
      } else {
        tagged.push(value.data);
      }
    }
    const variants = [...byTag.entries()]
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([tag, data]) => ({ tag, value: inferColumn(data) }));
    return Tag("TaggedUnion", { variants });
  }
  return inferRecordSchema(values as readonly Record<string, unknown>[], group.key);
}

type ValueGroup = {
  readonly key: string;
  readonly values: unknown[];
};

function groupValues(values: readonly unknown[]): ValueGroup[] {
  const groups = new Map<string, unknown[]>();
  for (const value of values) {
    const key = valueKind(value);
    const group = groups.get(key);
    if (group === undefined) {
      groups.set(key, [value]);
    } else {
      group.push(value);
    }
  }
  return [...groups.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([key, grouped]) => ({ key, values: grouped }));
}

function valueKind(value: unknown): string {
  if (typeof value === "number") {
    return "Number";
  }
  if (typeof value === "boolean") {
    return "Boolean";
  }
  if (typeof value === "string") {
    return "String";
  }
  if (value instanceof Date) {
    return "Date";
  }
  if (value instanceof Uint8Array) {
    return "Bytes";
  }
  if (Array.isArray(value)) {
    return "List";
  }
  if (isTaggedValue(value)) {
    return "TaggedUnion";
  }
  return Object.getPrototypeOf(value) === null ? "RecordNull" : "RecordObject";
}

function isTaggedValue(
  value: unknown,
): value is { readonly tag: string; readonly data: unknown } {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const keys = Object.keys(value);
  const tag = Object.getOwnPropertyDescriptor(value, "tag");
  const data = Object.getOwnPropertyDescriptor(value, "data");
  return keys.length === 2 && keys.includes("data") && keys.includes("tag") &&
    tag !== undefined && "value" in tag && typeof tag.value === "string" &&
    data !== undefined && "value" in data;
}

function inferStringSchema(values: readonly string[]): WireSchema {
  const unique = [...new Set(values)];
  const dictionaryCodeBytes = codeWidth(unique.length);
  const rawBytes = textEncoder.encode(values.join("")).byteLength;
  const dictionaryBytes = textEncoder.encode(unique.join("")).byteLength;
  const rawEstimate = (values.length + 1) * 4 + rawBytes;
  const dictionaryEstimate =
    (unique.length + 1) * 4 + dictionaryBytes + values.length * dictionaryCodeBytes;
  return dictionaryEstimate < rawEstimate
    ? Tag("StringDictionary", { codeBytes: dictionaryCodeBytes })
    : Tag("StringUtf8");
}

function inferRecordSchema(
  values: readonly Record<string, unknown>[],
  kind: string,
): WireSchema {
  const names = [...new Set(values.flatMap((value) => Object.keys(value)))].sort();
  const fields = names.map((name): WireField => {
    const presentValues: unknown[] = [];
    let optional = false;
    for (const value of values) {
      const descriptor = Object.getOwnPropertyDescriptor(value, name);
      if (descriptor === undefined || !("value" in descriptor)) {
        optional = true;
      } else {
        presentValues.push(descriptor.value);
      }
    }
    return { name, optional, value: inferColumn(presentValues) };
  });
  return Tag("Record", {
    prototype: kind === "RecordNull" ? "Null" as const : "Object" as const,
    fields,
  });
}

function encodeColumn(
  schema: WireSchema,
  values: readonly unknown[],
  writer: BinaryWriter,
  state: EncodingState,
  depth: number,
): void {
  if (depth > MAXIMUM_INFERENCE_DEPTH) {
    throw new TypeError("packed value exceeds the maximum nesting depth");
  }
  writer.u32([values.length]);
  if (schema.tag === "Empty") {
    if (values.length !== 0) {
      throw new TypeError("empty schema received values");
    }
    return;
  }
  if (schema.tag === "Null") {
    if (values.some((value) => value !== null)) {
      throw new TypeError("null schema received a non-null value");
    }
    return;
  }
  if (schema.tag === "Undefined") {
    if (values.some((value) => value !== undefined)) {
      throw new TypeError("undefined schema received a defined value");
    }
    return;
  }
  if (schema.tag === "Number") {
    requireValues(values, (value) => typeof value === "number", "number");
    writer.f64(values as readonly number[]);
    return;
  }
  if (schema.tag === "Boolean") {
    requireValues(values, (value) => typeof value === "boolean", "boolean");
    writer.bits(values as readonly boolean[]);
    return;
  }
  if (schema.tag === "StringUtf8") {
    requireValues(values, (value) => typeof value === "string", "string");
    encodeStrings(values as readonly string[], writer);
    return;
  }
  if (schema.tag === "StringDictionary") {
    requireValues(values, (value) => typeof value === "string", "string");
    encodeDictionaryStrings(values as readonly string[], schema.data.codeBytes, writer);
    return;
  }
  if (schema.tag === "Date") {
    requireValues(values, (value) => value instanceof Date, "Date");
    trackObjects(values as readonly object[], state);
    writer.f64((values as readonly Date[]).map((value) => value.getTime()));
    return;
  }
  if (schema.tag === "Bytes") {
    requireValues(values, (value) => value instanceof Uint8Array, "Uint8Array");
    trackObjects(values as readonly object[], state);
    encodeBytes(values as readonly Uint8Array[], writer);
    return;
  }
  if (schema.tag === "List") {
    const offsets = new Uint32Array(values.length + 1);
    const flattened: unknown[] = [];
    for (let index = 0; index < values.length; index += 1) {
      const candidate = values[index];
      if (!Array.isArray(candidate)) {
        throw new TypeError("list schema received a non-array value");
      }
      trackObject(candidate, state);
      requireDenseArray(candidate);
      const list = candidate as readonly unknown[];
      for (const item of list) {
        flattened.push(item);
      }
      offsets[index + 1] = flattened.length;
    }
    writer.u32(offsets);
    encodeColumn(schema.data.item, flattened, writer, state, depth + 1);
    return;
  }
  if (schema.tag === "Record") {
    encodeRecords(schema, values, writer, state, depth);
    return;
  }
  if (schema.tag === "Nullable") {
    const validity = values.map((value) => value !== null);
    writer.bits(validity);
    encodeColumn(
      schema.data.value,
      values.filter((value) => value !== null),
      writer,
      state,
      depth + 1,
    );
    return;
  }
  if (schema.tag === "MaybeUndefined") {
    const validity = values.map((value) => value !== undefined);
    writer.bits(validity);
    encodeColumn(
      schema.data.value,
      values.filter((value) => value !== undefined),
      writer,
      state,
      depth + 1,
    );
    return;
  }
  if (schema.tag === "Union") {
    encodeUnion(schema, values, writer, state, depth);
    return;
  }
  encodeTaggedUnion(schema, values, writer, state, depth);
}

function encodeStrings(values: readonly string[], writer: BinaryWriter): void {
  const offsets = new Uint32Array(values.length + 1);
  let joined = "";
  for (let index = 0; index < values.length; index += 1) {
    joined += values[index];
    offsets[index + 1] = joined.length;
  }
  const bytes = textEncoder.encode(joined);
  writer.u32(offsets);
  writer.u32([bytes.byteLength]);
  writer.bytes(bytes);
}

function encodeDictionaryStrings(
  values: readonly string[],
  width: 1 | 2 | 4,
  writer: BinaryWriter,
): void {
  const dictionary = [...new Set(values)];
  if (codeWidth(dictionary.length) > width) {
    throw new TypeError("string dictionary exceeds its inferred code width");
  }
  const indices = new Map(dictionary.map((value, index) => [value, index]));
  writer.u32([dictionary.length]);
  encodeStrings(dictionary, writer);
  writer.codes(values.map((value) => indices.get(value) as number), width);
}

function encodeBytes(values: readonly Uint8Array[], writer: BinaryWriter): void {
  const offsets = new Uint32Array(values.length + 1);
  let total = 0;
  for (let index = 0; index < values.length; index += 1) {
    total += values[index]?.byteLength ?? 0;
    offsets[index + 1] = total;
  }
  const arena = new Uint8Array(total);
  let offset = 0;
  for (const value of values) {
    arena.set(value, offset);
    offset += value.byteLength;
  }
  writer.u32(offsets);
  writer.bytes(arena);
}

function encodeRecords(
  schema: Extract<WireSchema, Tagged<"Record", unknown>>,
  candidates: readonly unknown[],
  writer: BinaryWriter,
  state: EncodingState,
  depth: number,
): void {
  const values: Record<string, unknown>[] = [];
  let expectedNames = recordFieldNames.get(schema);
  if (expectedNames === undefined) {
    expectedNames = new Set(schema.data.fields.map((field) => field.name));
    recordFieldNames.set(schema, expectedNames);
  }
  for (const candidate of candidates) {
    if (typeof candidate !== "object" || candidate === null || Array.isArray(candidate)) {
      throw new TypeError("record schema received a non-record value");
    }
    const prototype = Object.getPrototypeOf(candidate);
    if (
      (schema.data.prototype === "Object" && prototype !== Object.prototype) ||
      (schema.data.prototype === "Null" && prototype !== null)
    ) {
      throw new TypeError("record value has a different prototype from its schema");
    }
    trackObject(candidate, state);
    for (const name of Object.keys(candidate)) {
      if (!expectedNames.has(name)) {
        throw new TypeError("record value contains a field outside its schema");
      }
      const descriptor = Object.getOwnPropertyDescriptor(candidate, name);
      if (descriptor === undefined || !("value" in descriptor)) {
        throw new TypeError("record value contains an accessor property");
      }
    }
    values.push(candidate as Record<string, unknown>);
  }
  for (const field of schema.data.fields) {
    const present: boolean[] = [];
    const fieldValues: unknown[] = [];
    for (const value of values) {
      const hasValue = Object.hasOwn(value, field.name);
      if (!field.optional && !hasValue) {
        throw new TypeError("record value omitted a required field");
      }
      present.push(hasValue);
      if (hasValue) {
        fieldValues.push(value[field.name]);
      }
    }
    if (field.optional) {
      writer.bits(present);
    }
    encodeColumn(field.value, fieldValues, writer, state, depth + 1);
  }
}

function encodeUnion(
  schema: Extract<WireSchema, Tagged<"Union", unknown>>,
  values: readonly unknown[],
  writer: BinaryWriter,
  state: EncodingState,
  depth: number,
): void {
  const grouped = groupValues(values);
  const schemas = schema.data.variants;
  if (grouped.length !== schemas.length) {
    throw new TypeError("union groups did not match their inferred schema");
  }
  const byKind = new Map(grouped.map((group, index) => [group.key, index]));
  const codes: number[] = [];
  const variantValues = schemas.map((): unknown[] => []);
  for (const value of values) {
    const index = byKind.get(valueKind(value));
    if (index === undefined) {
      throw new TypeError("union value did not match its inferred schema");
    }
    codes.push(index);
    variantValues[index]?.push(value);
  }
  writer.codes(codes, schema.data.codeBytes);
  for (let index = 0; index < schemas.length; index += 1) {
    encodeColumn(
      schemas[index] as WireSchema,
      variantValues[index] as unknown[],
      writer,
      state,
      depth + 1,
    );
  }
}

function encodeTaggedUnion(
  schema: Extract<WireSchema, Tagged<"TaggedUnion", unknown>>,
  candidates: readonly unknown[],
  writer: BinaryWriter,
  state: EncodingState,
  depth: number,
): void {
  const byTag = new Map(schema.data.variants.map((variant, index) => [variant.tag, index]));
  const variantValues = schema.data.variants.map((): unknown[] => []);
  const codes: number[] = [];
  for (const candidate of candidates) {
    if (typeof candidate !== "object" || candidate === null) {
      throw new TypeError("tagged schema received a non-Casework value");
    }
    const keys = Object.keys(candidate);
    if (keys.length !== 2 || !keys.includes("tag") || !keys.includes("data")) {
      throw new TypeError("tagged schema received a non-Casework value");
    }
    const tagDescriptor = Object.getOwnPropertyDescriptor(candidate, "tag");
    const dataDescriptor = Object.getOwnPropertyDescriptor(candidate, "data");
    if (
      tagDescriptor === undefined || !("value" in tagDescriptor) ||
      dataDescriptor === undefined || !("value" in dataDescriptor) ||
      typeof tagDescriptor.value !== "string"
    ) {
      throw new TypeError("tagged schema received a non-Casework value");
    }
    trackObject(candidate, state);
    const value = candidate as { readonly tag: string; readonly data: unknown };
    const index = byTag.get(value.tag);
    if (index === undefined) {
      throw new TypeError("tagged value was absent from its inferred schema");
    }
    codes.push(index);
    variantValues[index]?.push(value.data);
  }
  writer.codes(codes, codeWidth(schema.data.variants.length));
  for (let index = 0; index < schema.data.variants.length; index += 1) {
    encodeColumn(
      schema.data.variants[index]?.value as WireSchema,
      variantValues[index] as unknown[],
      writer,
      state,
      depth + 1,
    );
  }
}

type EncodingState = {
  readonly generation: number;
};

function requireValues(
  values: readonly unknown[],
  accepts: (value: unknown) => boolean,
  expected: string,
): void {
  if (values.some((value) => !accepts(value))) {
    throw new TypeError(`${expected} schema received an incompatible value`);
  }
}

function trackObjects(values: readonly object[], state: EncodingState): void {
  for (const value of values) {
    trackObject(value, state);
  }
}

function trackObject(value: object, state: EncodingState): void {
  if (encodingGenerations.get(value) === state.generation) {
    throw new TypeError("packed value contains shared or cyclic object identity");
  }
  encodingGenerations.set(value, state.generation);
}

function requireDenseArray(value: readonly unknown[]): void {
  const ownNames = Object.keys(value);
  for (const symbol of Object.getOwnPropertySymbols(value)) {
    if (Object.getOwnPropertyDescriptor(value, symbol)?.enumerable === true) {
      throw new TypeError("packed list contains an enumerable symbol property");
    }
  }
  for (let index = 0; index < value.length; index += 1) {
    const descriptor = Object.getOwnPropertyDescriptor(value, index);
    if (descriptor === undefined) {
      throw new TypeError("packed list contains an array hole");
    }
    if (!("value" in descriptor)) {
      throw new TypeError("packed list contains an accessor element");
    }
  }
  if (ownNames.some((name) => !isArrayIndex(name, value.length))) {
    throw new TypeError("packed list contains an enumerable array property");
  }
}

function decodeColumn(schema: WireSchema, reader: BinaryReader): unknown[] {
  const count = reader.u32(1)[0] ?? 0;
  if (schema.tag === "Empty") {
    if (count !== 0) {
      throw new TypeError("empty schema contained values");
    }
    return [];
  }
  if (schema.tag === "Null") {
    return new Array(count).fill(null);
  }
  if (schema.tag === "Undefined") {
    return new Array(count).fill(undefined);
  }
  if (schema.tag === "Number") {
    return [...reader.f64(count)];
  }
  if (schema.tag === "Boolean") {
    return reader.bits(count);
  }
  if (schema.tag === "StringUtf8") {
    return decodeStrings(count, reader);
  }
  if (schema.tag === "StringDictionary") {
    const dictionaryCount = reader.u32(1)[0] ?? 0;
    const dictionary = decodeStrings(dictionaryCount, reader);
    return reader.codes(count, schema.data.codeBytes).map((code) => {
      const value = dictionary[code];
      if (value === undefined) {
        throw new TypeError("dictionary string code is outside its dictionary");
      }
      return value;
    });
  }
  if (schema.tag === "Date") {
    return [...reader.f64(count)].map((value) => new Date(value));
  }
  if (schema.tag === "Bytes") {
    const offsets = reader.u32(count + 1);
    const total = offsets[count] ?? 0;
    const arena = reader.bytes(total);
    return offsetsToValues(offsets, (start, end) => arena.slice(start, end));
  }
  if (schema.tag === "List") {
    const offsets = reader.u32(count + 1);
    const flattened = decodeColumn(schema.data.item, reader);
    requireOffsetTotal(offsets, flattened.length);
    return offsetsToValues(offsets, (start, end) => flattened.slice(start, end));
  }
  if (schema.tag === "Record") {
    return decodeRecords(schema, count, reader);
  }
  if (schema.tag === "Nullable") {
    return decodePresence(count, null, schema.data.value, reader);
  }
  if (schema.tag === "MaybeUndefined") {
    return decodePresence(count, undefined, schema.data.value, reader);
  }
  if (schema.tag === "Union") {
    return decodeUnion(schema, count, reader);
  }
  return decodeTaggedUnion(schema, count, reader);
}

function decodeStrings(count: number, reader: BinaryReader): string[] {
  const offsets = reader.u32(count + 1);
  const byteLength = reader.u32(1)[0] ?? 0;
  const joined = textDecoder.decode(reader.bytes(byteLength));
  requireOffsetTotal(offsets, joined.length);
  return offsetsToValues(offsets, (start, end) => joined.substring(start, end));
}

function decodeRecords(
  schema: Extract<WireSchema, Tagged<"Record", unknown>>,
  count: number,
  reader: BinaryReader,
): Record<string, unknown>[] {
  const records = Array.from({ length: count }, () =>
    schema.data.prototype === "Null" ? Object.create(null) as Record<string, unknown> : {}
  );
  for (const field of schema.data.fields) {
    const present = field.optional ? reader.bits(count) : new Array(count).fill(true) as boolean[];
    const values = decodeColumn(field.value, reader);
    let valueIndex = 0;
    for (let index = 0; index < count; index += 1) {
      if (present[index] === true) {
        Object.defineProperty(records[index]!, field.name, {
          value: values[valueIndex],
          enumerable: true,
          configurable: true,
          writable: true,
        });
        valueIndex += 1;
      }
    }
    if (valueIndex !== values.length) {
      throw new TypeError("record field count did not match its presence plane");
    }
  }
  return records;
}

function decodePresence(
  count: number,
  absent: null | undefined,
  valueSchema: WireSchema,
  reader: BinaryReader,
): unknown[] {
  const present = reader.bits(count);
  const values = decodeColumn(valueSchema, reader);
  const decoded: unknown[] = new Array(count);
  let valueIndex = 0;
  for (let index = 0; index < count; index += 1) {
    if (present[index] === true) {
      decoded[index] = values[valueIndex];
      valueIndex += 1;
    } else {
      decoded[index] = absent;
    }
  }
  if (valueIndex !== values.length) {
    throw new TypeError("presence plane did not match its value count");
  }
  return decoded;
}

function decodeUnion(
  schema: Extract<WireSchema, Tagged<"Union", unknown>>,
  count: number,
  reader: BinaryReader,
): unknown[] {
  const codes = reader.codes(count, schema.data.codeBytes);
  const variants = schema.data.variants.map((variant) => decodeColumn(variant, reader));
  const cursors = schema.data.variants.map(() => 0);
  return codes.map((code) => {
    const values = variants[code];
    const cursor = cursors[code];
    if (values === undefined || cursor === undefined || cursor >= values.length) {
      throw new TypeError("union code is outside its decoded variants");
    }
    cursors[code] = cursor + 1;
    return values[cursor];
  });
}

function decodeTaggedUnion(
  schema: Extract<WireSchema, Tagged<"TaggedUnion", unknown>>,
  count: number,
  reader: BinaryReader,
): unknown[] {
  const codes = reader.codes(count, codeWidth(schema.data.variants.length));
  const variants = schema.data.variants.map((variant) => decodeColumn(variant.value, reader));
  const cursors = schema.data.variants.map(() => 0);
  return codes.map((code) => {
    const variant = schema.data.variants[code];
    const values = variants[code];
    const cursor = cursors[code];
    if (variant === undefined || values === undefined || cursor === undefined || cursor >= values.length) {
      throw new TypeError("tag code is outside its decoded variants");
    }
    cursors[code] = cursor + 1;
    return Tag(variant.tag as never, values[cursor]);
  });
}

function offsetsToValues<Value>(
  offsets: Uint32Array,
  make: (start: number, end: number) => Value,
): Value[] {
  const values: Value[] = new Array(Math.max(0, offsets.length - 1));
  for (let index = 0; index + 1 < offsets.length; index += 1) {
    const start = offsets[index] ?? 0;
    const end = offsets[index + 1] ?? 0;
    if (end < start) {
      throw new TypeError("packed offsets are not monotonic");
    }
    values[index] = make(start, end);
  }
  return values;
}

function requireOffsetTotal(offsets: Uint32Array, expected: number): void {
  if ((offsets[offsets.length - 1] ?? 0) !== expected) {
    throw new TypeError("packed offsets do not consume their value arena");
  }
}

function codeWidth(count: number): 1 | 2 | 4 {
  return count <= 0x100 ? 1 : count <= 0x1_0000 ? 2 : 4;
}

type BinaryPart = {
  readonly alignment: 1 | 2 | 4 | 8;
  readonly bytes: Uint8Array;
};

class BinaryWriter {
  readonly #parts: BinaryPart[] = [];

  u32(values: ArrayLike<number>): void {
    const typed = Uint32Array.from(values);
    this.#parts.push({ alignment: 4, bytes: new Uint8Array(typed.buffer) });
  }

  f64(values: ArrayLike<number>): void {
    const typed = Float64Array.from(values);
    this.#parts.push({ alignment: 8, bytes: new Uint8Array(typed.buffer) });
  }

  bits(values: readonly boolean[]): void {
    const words = new Uint32Array(Math.ceil(values.length / 32));
    for (let index = 0; index < values.length; index += 1) {
      if (values[index] === true) {
        words[index >>> 5] = (words[index >>> 5] ?? 0) | (1 << (index & 31));
      }
    }
    this.#parts.push({ alignment: 4, bytes: new Uint8Array(words.buffer) });
  }

  codes(values: readonly number[], width: 1 | 2 | 4): void {
    if (width === 1) {
      this.#parts.push({ alignment: 1, bytes: Uint8Array.from(values) });
      return;
    }
    if (width === 2) {
      const typed = Uint16Array.from(values);
      this.#parts.push({ alignment: 2, bytes: new Uint8Array(typed.buffer) });
      return;
    }
    this.u32(values);
  }

  bytes(values: Uint8Array): void {
    this.#parts.push({ alignment: 1, bytes: values });
  }

  finish(): ArrayBuffer {
    let byteLength = 0;
    for (const part of this.#parts) {
      byteLength = align(byteLength, part.alignment) + part.bytes.byteLength;
    }
    const buffer = new ArrayBuffer(byteLength);
    const output = new Uint8Array(buffer);
    let offset = 0;
    for (const part of this.#parts) {
      offset = align(offset, part.alignment);
      output.set(part.bytes, offset);
      offset += part.bytes.byteLength;
    }
    return buffer;
  }
}

class BinaryReader {
  readonly #buffer: ArrayBuffer;
  #offset = 0;

  constructor(buffer: ArrayBuffer) {
    this.#buffer = buffer;
  }

  u32(count: number): Uint32Array {
    this.#offset = align(this.#offset, 4);
    const values = new Uint32Array(this.#buffer, this.#offset, count);
    this.#offset += values.byteLength;
    return values;
  }

  f64(count: number): Float64Array {
    this.#offset = align(this.#offset, 8);
    const values = new Float64Array(this.#buffer, this.#offset, count);
    this.#offset += values.byteLength;
    return values;
  }

  bits(count: number): boolean[] {
    const words = this.u32(Math.ceil(count / 32));
    return Array.from({ length: count }, (_, index) =>
      ((words[index >>> 5] ?? 0) & (1 << (index & 31))) !== 0
    );
  }

  codes(count: number, width: 1 | 2 | 4): number[] {
    if (width === 1) {
      return [...this.bytes(count)];
    }
    if (width === 2) {
      this.#offset = align(this.#offset, 2);
      const values = new Uint16Array(this.#buffer, this.#offset, count);
      this.#offset += values.byteLength;
      return [...values];
    }
    return [...this.u32(count)];
  }

  bytes(count: number): Uint8Array {
    if (!Number.isSafeInteger(count) || count < 0 || count > this.#buffer.byteLength - this.#offset) {
      throw new TypeError("packed byte range is outside its frame");
    }
    const values = new Uint8Array(this.#buffer, this.#offset, count);
    this.#offset += count;
    return values;
  }

  requireFinished(): void {
    if (this.#offset !== this.#buffer.byteLength) {
      throw new TypeError("packed frame contains trailing bytes");
    }
  }
}

function align(offset: number, alignment: 1 | 2 | 4 | 8): number {
  return Math.ceil(offset / alignment) * alignment;
}

function boundedAdd(left: number, right: number): number {
  const sum = left + right;
  return Number.isSafeInteger(sum) ? sum : Number.MAX_SAFE_INTEGER;
}

function assertLittleEndian(): void {
  const buffer = new ArrayBuffer(2);
  new Uint16Array(buffer)[0] = 0x0102;
  if (new Uint8Array(buffer)[0] !== 0x02) {
    throw new TypeError("packed worker wire requires a little-endian JavaScript engine");
  }
}
