import { Tag } from "../casework.js";
import type { Tag as Tagged } from "../casework.js";

export type ClonedWireBatch = Tagged<
  "ClonedBatch",
  { readonly values: readonly unknown[] }
>;

export type CodecWireBatch = Tagged<
  "CodecBatch",
  {
    readonly count: number;
    readonly codec: string;
    readonly payload: ArrayBuffer;
  }
>;

export type WireBatch = ClonedWireBatch | CodecWireBatch;

export type EncodedWireBatch = {
  readonly message: WireBatch;
  readonly transfer: readonly Transferable[];
};

export const MAXIMUM_WIRE_BATCH_ITEMS = 4_096;

export type WireCodecPolicy = {
  readonly minimumCodecItems?: number;
};

export type WireCodec<Value> = {
  readonly id: string;
  readonly accepts?: (values: readonly Value[]) => boolean;
  readonly encode: (values: readonly Value[]) => ArrayBuffer;
  readonly decode: (buffer: ArrayBuffer) => readonly Value[];
};

export type WireBatchEncoderOptions<Value> = WireCodecPolicy & {
  readonly codec?: WireCodec<Value>;
};

export class WireBatchEncoder<Value = unknown> {
  readonly #minimumCodecItems: number;
  readonly #codec: WireCodec<Value> | undefined;

  constructor(options: WireBatchEncoderOptions<Value> = {}) {
    this.#codec = options.codec;
    this.#minimumCodecItems = options.minimumCodecItems ?? 1;
    if (
      !Number.isSafeInteger(this.#minimumCodecItems) ||
      this.#minimumCodecItems <= 0
    ) {
      throw new TypeError("wire codec item threshold must be a positive integer");
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
    if (
      this.#codec === undefined ||
      values.length < this.#minimumCodecItems ||
      (this.#codec.accepts !== undefined && !this.#codec.accepts(values))
    ) {
      return { message: Tag("ClonedBatch", { values }), transfer: [] };
    }
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
}

export class WireBatchDecoder<Value = unknown> {
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
    if (
      !Number.isSafeInteger(message.data.count) ||
      message.data.count < 0 ||
      message.data.count > MAXIMUM_WIRE_BATCH_ITEMS ||
      typeof message.data.codec !== "string" ||
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
}
