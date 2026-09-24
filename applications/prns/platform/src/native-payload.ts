/** A malformed platform capability payload (Rust values use generated codecs). */
export class NativePayloadError extends Error {
  readonly path: string;

  constructor(path: string, detail: string, options?: ErrorOptions) {
    super(`Invalid native payload at ${path}: ${detail}`, options);
    this.name = "NativePayloadError";
    this.path = path;
  }
}
