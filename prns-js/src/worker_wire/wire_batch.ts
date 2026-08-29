import { Tag } from "../casework.js";
import type { Tag as Tagged } from "../casework.js";
import {
  decodePackedValue,
  inferPackedValue,
  packWireValue,
  packingSupport,
} from "./inferred_codec.js";
import type { PackedValue, WireSchema } from "./inferred_codec.js";

export type ClonedWireBatch = Tagged<
  "ClonedBatch",
  { readonly values: readonly unknown[] }
>;

export type PackedWireBatch = Tagged<
  "PackedBatch",
  {
    readonly count: number;
    readonly cloned: readonly WireCloneEntry[];
    readonly packedIndices: ArrayBuffer;
    readonly schemaId: number;
    readonly schema?: WireSchema;
    readonly fingerprint?: string;
    readonly payload: ArrayBuffer;
  }
>;

export type CodecWireBatch = Tagged<
  "CodecBatch",
  {
    readonly count: number;
    readonly codec: string;
    readonly payload: ArrayBuffer;
  }
>;

export type WireBatch = ClonedWireBatch | PackedWireBatch | CodecWireBatch;

export type WireCloneEntry = {
  readonly index: number;
  readonly value: unknown;
};

export type EncodedWireBatch = {
  readonly message: WireBatch;
  readonly transfer: readonly Transferable[];
};

const DEFAULT_MINIMUM_PACKED_ITEMS = Number.MAX_SAFE_INTEGER;
const MAXIMUM_SCHEMAS = 256;
const MAXIMUM_SCHEMA_BYTES = 64 * 1024;
export const MAXIMUM_WIRE_BATCH_ITEMS = 4_096;

type EncoderSchema = {
  readonly id: number;
  readonly schema: WireSchema;
  announced: boolean;
};

export type WirePackingPolicy = {
  readonly minimumItems?: number;
};

export type WireCodec<Value> = {
  readonly id: string;
  readonly accepts?: (values: readonly Value[]) => boolean;
  readonly encode: (values: readonly Value[]) => ArrayBuffer;
  readonly decode: (buffer: ArrayBuffer) => readonly Value[];
};

export type WireBatchEncoderOptions<Value> = WirePackingPolicy & {
  readonly codec?: WireCodec<Value>;
};

export class WireBatchEncoder<Value = unknown> {
  readonly #schemas = new Map<string, EncoderSchema>();
  readonly #minimumItems: number;
  readonly #codec: WireCodec<Value> | undefined;
  #warmSchema: { readonly schema: WireSchema; readonly fingerprint: string } | undefined;
  #nextSchemaId = 1;

  constructor(options: WireBatchEncoderOptions<Value> = {}) {
    this.#codec = options.codec;
    this.#minimumItems = options.minimumItems ??
      (options.codec === undefined ? DEFAULT_MINIMUM_PACKED_ITEMS : 1);
    if (!Number.isSafeInteger(this.#minimumItems) || this.#minimumItems <= 0) {
      throw new TypeError("wire packing item threshold must be a positive integer");
    }
    if (
      this.#codec !== undefined &&
      (this.#codec.id.length === 0 || this.#codec.id.length > 64)
    ) {
      throw new TypeError("wire codec id must contain between 1 and 64 characters");
    }
  }

  encode(values: readonly Value[]): EncodedWireBatch {
    if (values.length > MAXIMUM_WIRE_BATCH_ITEMS) {
      throw new TypeError("wire batch exceeds its item bound");
    }
    if (values.length === 0) {
      return { message: Tag("ClonedBatch", { values }), transfer: [] };
    }
    if (values.length < this.#minimumItems) {
      return { message: Tag("ClonedBatch", { values }), transfer: [] };
    }
    if (
      this.#codec !== undefined &&
      this.#codec.accepts !== undefined &&
      !this.#codec.accepts(values)
    ) {
      return { message: Tag("ClonedBatch", { values }), transfer: [] };
    }
    if (this.#codec !== undefined) {
      const payload = this.#codec.encode(values);
      if (!(payload instanceof ArrayBuffer)) {
        throw new TypeError("wire codec did not return an ArrayBuffer");
      }
      return {
        message: Tag("CodecBatch", {
          count: values.length,
          codec: this.#codec.id,
          payload,
        }),
        transfer: [payload],
      };
    }
    const packedValues: unknown[] = [];
    const packedIndices: number[] = [];
    const cloned: WireCloneEntry[] = [];
    let entireBatch;
    if (this.#warmSchema === undefined) {
      entireBatch = inferPackedValue(values);
    } else {
      try {
        entireBatch = Tag("Packed", packWireValue(this.#warmSchema.schema, values));
      } catch {
        entireBatch = inferPackedValue(values);
      }
    }
    let packed: PackedValue;
    if (entireBatch.tag === "Packed") {
      packed = entireBatch.data;
      this.#warmSchema = {
        schema: packed.schema,
        fingerprint: packed.fingerprint,
      };
      for (let index = 0; index < values.length; index += 1) {
        packedValues.push(values[index]);
        packedIndices.push(index);
      }
    } else {
      for (let index = 0; index < values.length; index += 1) {
        const value = values[index];
        if (packingSupport(value).tag === "Supported") {
          packedValues.push(value);
          packedIndices.push(index);
        } else {
          cloned.push({ index, value });
        }
      }
      if (packedValues.length === 0) {
        return { message: Tag("ClonedBatch", { values }), transfer: [] };
      }
      const partialBatch = inferPackedValue(packedValues);
      if (partialBatch.tag !== "Packed") {
        return { message: Tag("ClonedBatch", { values }), transfer: [] };
      }
      packed = partialBatch.data;
    }
    if (packedValues.length < this.#minimumItems) {
      return { message: Tag("ClonedBatch", { values }), transfer: [] };
    }
    const schemaBytes = packed.fingerprint.length * 3;
    if (schemaBytes > MAXIMUM_SCHEMA_BYTES) {
      return { message: Tag("ClonedBatch", { values }), transfer: [] };
    }
    let registered = this.#schemas.get(packed.fingerprint);
    if (registered === undefined) {
      if (this.#schemas.size === MAXIMUM_SCHEMAS) {
        return { message: Tag("ClonedBatch", { values }), transfer: [] };
      }
      registered = {
        id: this.#nextSchemaId,
        schema: packed.schema,
        announced: false,
      };
      this.#nextSchemaId += 1;
      this.#schemas.set(packed.fingerprint, registered);
    }
    const indices = Uint32Array.from(packedIndices).buffer;
    const definition = registered.announced
      ? {}
      : { schema: packed.schema, fingerprint: packed.fingerprint };
    registered.announced = true;
    return {
      message: Tag("PackedBatch", {
        count: values.length,
        cloned,
        packedIndices: indices,
        schemaId: registered.id,
        ...definition,
        payload: packed.buffer,
      }),
      transfer: [indices, packed.buffer],
    };
  }
}

export class WireBatchDecoder<Value = unknown> {
  readonly #schemas = new Map<number, { readonly schema: WireSchema; readonly fingerprint: string }>();
  readonly #codecs = new Map<string, WireCodec<Value>>();

  constructor(codecs: readonly WireCodec<Value>[] = []) {
    for (const codec of codecs) {
      if (
        codec.id.length === 0 ||
        codec.id.length > 64 ||
        this.#codecs.has(codec.id)
      ) {
        throw new TypeError("wire codec registry contains an invalid or duplicate id");
      }
      this.#codecs.set(codec.id, codec);
    }
  }

  decode(message: WireBatch): readonly Value[] {
    if (message.tag === "ClonedBatch") {
      if (
        !Array.isArray(message.data.values) ||
        message.data.values.length > MAXIMUM_WIRE_BATCH_ITEMS
      ) {
        throw new TypeError("cloned batch exceeds its item bound");
      }
      return message.data.values as readonly Value[];
    }
    if (message.tag === "CodecBatch") {
      if (
        !Number.isSafeInteger(message.data.count) ||
        message.data.count < 0 ||
        message.data.count > MAXIMUM_WIRE_BATCH_ITEMS ||
        !(message.data.payload instanceof ArrayBuffer)
      ) {
        throw new TypeError("codec batch envelope is invalid");
      }
      const codec = this.#codecs.get(message.data.codec);
      if (codec === undefined) {
        throw new TypeError("codec batch references an unknown codec");
      }
      const values = codec.decode(message.data.payload);
      if (!Array.isArray(values) || values.length !== message.data.count) {
        throw new TypeError("wire codec decoded the wrong number of values");
      }
      return values;
    }
    if (
      !Number.isSafeInteger(message.data.count) ||
      message.data.count < 0 ||
      message.data.count > MAXIMUM_WIRE_BATCH_ITEMS ||
      !(message.data.packedIndices instanceof ArrayBuffer) ||
      !(message.data.payload instanceof ArrayBuffer) ||
      !Array.isArray(message.data.cloned) ||
      message.data.cloned.length > message.data.count
    ) {
      throw new TypeError("packed batch envelope is invalid");
    }
    const schema = this.#resolveSchema(message);
    const decoded = decodePackedValue(schema, message.data.payload);
    if (!Array.isArray(decoded)) {
      throw new TypeError("packed batch payload did not decode to an array");
    }
    const indices = new Uint32Array(message.data.packedIndices);
    if (indices.length !== decoded.length) {
      throw new TypeError("packed batch index count did not match its values");
    }
    const values: unknown[] = new Array(message.data.count);
    const occupied = new Uint8Array(message.data.count);
    for (let index = 0; index < indices.length; index += 1) {
      const target = indices[index];
      if (target === undefined || target >= values.length || occupied[target] !== 0) {
        throw new TypeError("packed batch contains an invalid value index");
      }
      values[target] = decoded[index];
      occupied[target] = 1;
    }
    for (const cloned of message.data.cloned) {
      if (
        !Number.isSafeInteger(cloned.index) ||
        cloned.index < 0 ||
        cloned.index >= values.length ||
        occupied[cloned.index] !== 0
      ) {
        throw new TypeError("packed batch contains an invalid clone index");
      }
      values[cloned.index] = cloned.value;
      occupied[cloned.index] = 1;
    }
    if (occupied.some((value) => value === 0)) {
      throw new TypeError("packed batch omitted one or more values");
    }
    return values as Value[];
  }

  #resolveSchema(message: PackedWireBatch): WireSchema {
    if (!Number.isSafeInteger(message.data.schemaId) || message.data.schemaId <= 0) {
      throw new TypeError("packed batch schema id is invalid");
    }
    const existing = this.#schemas.get(message.data.schemaId);
    if (message.data.schema === undefined || message.data.fingerprint === undefined) {
      if (existing === undefined) {
        throw new TypeError("packed batch references an unknown schema");
      }
      return existing.schema;
    }
    validateWireSchema(message.data.schema, 0, new WeakSet());
    if (textEncoder.encode(message.data.fingerprint).byteLength > MAXIMUM_SCHEMA_BYTES) {
      throw new TypeError("packed batch schema exceeds its descriptor bound");
    }
    if (message.data.fingerprint !== JSON.stringify(message.data.schema)) {
      throw new TypeError("packed batch schema fingerprint is invalid");
    }
    if (existing !== undefined) {
      if (existing.fingerprint !== message.data.fingerprint) {
        throw new TypeError("packed batch redefines an existing schema");
      }
      return existing.schema;
    }
    if (this.#schemas.size === MAXIMUM_SCHEMAS) {
      throw new TypeError("packed batch schema registry is full");
    }
    this.#schemas.set(message.data.schemaId, {
      schema: message.data.schema,
      fingerprint: message.data.fingerprint,
    });
    return message.data.schema;
  }
}

const textEncoder = new TextEncoder();

function validateWireSchema(
  value: unknown,
  depth: number,
  seen: WeakSet<object>,
): asserts value is WireSchema {
  if (depth > 32 || typeof value !== "object" || value === null || seen.has(value)) {
    throw new TypeError("packed batch schema is invalid");
  }
  seen.add(value);
  const tagged = value as { readonly tag?: unknown; readonly data?: unknown };
  if (typeof tagged.tag !== "string") {
    throw new TypeError("packed batch schema tag is invalid");
  }
  if (
    tagged.tag === "Empty" ||
    tagged.tag === "Null" ||
    tagged.tag === "Undefined" ||
    tagged.tag === "Number" ||
    tagged.tag === "Boolean" ||
    tagged.tag === "StringUtf8" ||
    tagged.tag === "Date" ||
    tagged.tag === "Bytes"
  ) {
    if (tagged.data !== undefined) {
      throw new TypeError("packed batch unit schema contains data");
    }
    return;
  }
  if (tagged.tag === "StringDictionary") {
    requireCodeWidth((tagged.data as { readonly codeBytes?: unknown } | undefined)?.codeBytes);
    return;
  }
  if (tagged.tag === "List") {
    validateWireSchema((tagged.data as { readonly item?: unknown } | undefined)?.item, depth + 1, seen);
    return;
  }
  if (tagged.tag === "Nullable" || tagged.tag === "MaybeUndefined") {
    validateWireSchema((tagged.data as { readonly value?: unknown } | undefined)?.value, depth + 1, seen);
    return;
  }
  if (tagged.tag === "Record") {
    const data = tagged.data as {
      readonly prototype?: unknown;
      readonly fields?: unknown;
    } | undefined;
    if (
      (data?.prototype !== "Object" && data?.prototype !== "Null") ||
      !Array.isArray(data.fields)
    ) {
      throw new TypeError("packed batch record schema is invalid");
    }
    const names = new Set<string>();
    for (const field of data.fields) {
      if (typeof field !== "object" || field === null) {
        throw new TypeError("packed batch record field is invalid");
      }
      const candidate = field as {
        readonly name?: unknown;
        readonly optional?: unknown;
        readonly value?: unknown;
      };
      if (
        typeof candidate.name !== "string" ||
        names.has(candidate.name) ||
        typeof candidate.optional !== "boolean"
      ) {
        throw new TypeError("packed batch record field is invalid");
      }
      names.add(candidate.name);
      validateWireSchema(candidate.value, depth + 1, seen);
    }
    return;
  }
  if (tagged.tag === "Union") {
    const data = tagged.data as {
      readonly codeBytes?: unknown;
      readonly variants?: unknown;
    } | undefined;
    requireCodeWidth(data?.codeBytes);
    if (!Array.isArray(data?.variants) || data.variants.length < 2) {
      throw new TypeError("packed batch union schema is invalid");
    }
    for (const variant of data.variants) {
      validateWireSchema(variant, depth + 1, seen);
    }
    return;
  }
  if (tagged.tag === "TaggedUnion") {
    const variants = (tagged.data as { readonly variants?: unknown } | undefined)?.variants;
    if (!Array.isArray(variants) || variants.length === 0) {
      throw new TypeError("packed batch tagged schema is invalid");
    }
    const tags = new Set<string>();
    for (const variant of variants) {
      if (typeof variant !== "object" || variant === null) {
        throw new TypeError("packed batch tagged variant is invalid");
      }
      const candidate = variant as { readonly tag?: unknown; readonly value?: unknown };
      if (typeof candidate.tag !== "string" || tags.has(candidate.tag)) {
        throw new TypeError("packed batch tagged variant is invalid");
      }
      tags.add(candidate.tag);
      validateWireSchema(candidate.value, depth + 1, seen);
    }
    return;
  }
  throw new TypeError("packed batch schema tag is unknown");
}

function requireCodeWidth(value: unknown): asserts value is 1 | 2 | 4 {
  if (value !== 1 && value !== 2 && value !== 4) {
    throw new TypeError("packed batch schema code width is invalid");
  }
}
