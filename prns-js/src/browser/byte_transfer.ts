export type TransferredByteSpan = {
  readonly bufferIndex: number;
  readonly byteOffset: number;
  readonly byteLength: number;
};

export type TransferredByteBatch = {
  readonly buffers: readonly ArrayBuffer[];
  readonly spans: readonly TransferredByteSpan[];
};

const NO_RETAINED_BUFFERS: ReadonlySet<ArrayBufferLike> = new Set();

export function prepareByteTransfer(
  values: readonly Uint8Array[],
  retainedBuffers: ReadonlySet<ArrayBufferLike> = NO_RETAINED_BUFFERS,
): TransferredByteBatch {
  const buffers: ArrayBuffer[] = [];
  const bufferIndices = new Map<ArrayBuffer, number>();
  const spans = new Array<TransferredByteSpan>(values.length);
  const copiedIndices: number[] = [];
  let copiedBytes = 0;
  for (let index = 0; index < values.length; index += 1) {
    const value = values[index];
    if (value === undefined || value.byteLength === 0) {
      throw new TypeError("byte transfer contains an empty value");
    }
    const buffer = value.buffer;
    if (buffer instanceof ArrayBuffer && !retainedBuffers.has(buffer)) {
      let bufferIndex = bufferIndices.get(buffer);
      if (bufferIndex === undefined) {
        bufferIndex = buffers.length;
        buffers.push(buffer);
        bufferIndices.set(buffer, bufferIndex);
      }
      spans[index] = {
        bufferIndex,
        byteOffset: value.byteOffset,
        byteLength: value.byteLength,
      };
      continue;
    }
    copiedIndices.push(index);
    copiedBytes += value.byteLength;
    if (!Number.isSafeInteger(copiedBytes)) {
      throw new RangeError("byte transfer copy exceeds the supported size");
    }
  }
  if (copiedIndices.length > 0) {
    const copied = new ArrayBuffer(copiedBytes);
    const copiedView = new Uint8Array(copied);
    const bufferIndex = buffers.length;
    buffers.push(copied);
    let byteOffset = 0;
    for (const index of copiedIndices) {
      const value = values[index];
      if (value === undefined) {
        throw new TypeError("byte transfer contains a missing value");
      }
      copiedView.set(value, byteOffset);
      spans[index] = {
        bufferIndex,
        byteOffset,
        byteLength: value.byteLength,
      };
      byteOffset += value.byteLength;
    }
  }
  return { buffers, spans };
}

export function receiveByteTransfer(
  batch: TransferredByteBatch,
): readonly Uint8Array[] {
  if (!Array.isArray(batch.buffers) || !Array.isArray(batch.spans)) {
    throw new TypeError("byte transfer is malformed");
  }
  return batch.spans.map((span) => {
    if (
      !Number.isSafeInteger(span.bufferIndex) ||
      span.bufferIndex < 0 ||
      span.bufferIndex >= batch.buffers.length ||
      !Number.isSafeInteger(span.byteOffset) ||
      span.byteOffset < 0 ||
      !Number.isSafeInteger(span.byteLength) ||
      span.byteLength <= 0
    ) {
      throw new TypeError("byte transfer span is invalid");
    }
    const buffer = batch.buffers[span.bufferIndex];
    if (
      !(buffer instanceof ArrayBuffer) ||
      span.byteOffset > buffer.byteLength - span.byteLength
    ) {
      throw new TypeError("byte transfer span exceeds its buffer");
    }
    return new Uint8Array(buffer, span.byteOffset, span.byteLength);
  });
}
