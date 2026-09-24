import * as C from "personal-rns/contract";
import type * as R from "personal-rns/remote-control";
import * as RA from "./generated/remote-control-adapter.generated";
import * as N from "./bindings.native";
import * as A from "./generated/host-adapter.generated";
import { HostEventIterator } from "./stream";

export * from "personal-rns/contract";
export type * from "personal-rns/remote-control";
export { defaultStoragePath } from "./platform.native";
export { HostEventIterator } from "./stream";
export type { RemoteControlOperations } from "./generated/remote-control-adapter.generated";
export type HostApplicationEvent = C.ApplicationEvent | { readonly tag: "RemoteControl"; readonly data: R.RemoteControlNativeRemoteControlEvent };

/** A command/query lease. It cannot stop a host owned by native composition. */
export class HostClient {
  readonly #native: N.HostClientHandle;
  readonly remoteControl: RA.RemoteControlOperations;
  readonly #streams = new Set<{ close(): void }>();
  #released = false;

  constructor(host: N.HostClientHandleLike) {
    if (!(host instanceof N.HostClientHandle)) throw new TypeError("Expected a generated PRNS Host object");
    this.#native = host;
    this.remoteControl = RA.bindRemoteControlOperations(() => this.#requireOpen());
  }
  #requireOpen(): N.HostClientHandle {
    if (this.#released) throw new Error("Host client released");
    return this.#native;
  }
  async execute(command: C.HostCommand, options?: { signal: AbortSignal }): Promise<C.CommandSettlement> {
    return A.liftCommandSettlement(await this.#requireOpen().execute(A.lowerHostCommand(command), options));
  }
  async remoteControlExchange(linkId: C.LinkId, request: R.RemoteControlRequest, options?: { signal: AbortSignal }): Promise<R.RemoteControlExchangeSettlement> {
    return RA.liftRemoteControlExchangeSettlement(await this.#requireOpen().remoteControlExchange(linkId, RA.lowerRemoteControlRequest(request), options));
  }
  async remoteControlTargetExchange(target: C.IdentityHash, request: R.RemoteControlRequest, options?: { signal: AbortSignal }): Promise<R.RemoteControlExchangeSettlement> {
    return RA.liftRemoteControlExchangeSettlement(await this.#requireOpen().remoteControlTargetExchange(target, RA.lowerRemoteControlRequest(request), options));
  }
  async snapshot(options?: { signal: AbortSignal }): Promise<C.HostSnapshot> {
    return A.liftHostSnapshot(await this.#requireOpen().snapshot(options));
  }
  lifecycle(): C.LifecycleSnapshot { return A.liftLifecycleSnapshot(this.#requireOpen().lifecycle()); }
  identityHash(): C.IdentityHash { return C.identityHash(this.#requireOpen().identityHash()); }
  destinationHashes(): readonly C.DestinationHash[] { return this.#requireOpen().destinationHashes().map(C.destinationHash); }

  applicationEvents(options: { signal?: AbortSignal } = {}): AsyncIterableIterator<HostApplicationEvent> {
    let stream: HostEventIterator<N.HostEvent, HostApplicationEvent> | undefined;
    stream = new HostEventIterator(nativeLane(this.#requireOpen().applicationEvents()), (value) => {
      if (value.tag === N.HostEvent_Tags.RemoteControl) return { tag: "RemoteControl", data: RA.liftRemoteControlNativeRemoteControlEvent(value.inner.event) };
      if (value.tag !== N.HostEvent_Tags.Application) throw new Error("Unexpected diagnostic in application lane");
      return A.liftApplicationEvent(value.inner.event, (descriptor) => {
        const reader = value.inner.resource;
        if (reader === undefined) throw new Error("Resource event has no native reader");
        const resource = new ReceivedResource(reader, descriptor.totalBytes);
        return resource;
      });
    }, options.signal, () => { if (stream !== undefined) this.#streams.delete(stream); });
    if (!stream.closed) this.#streams.add(stream);
    return stream;
  }
  diagnostics(options: { signal?: AbortSignal } = {}): AsyncIterableIterator<C.DiagnosticEvent> {
    let stream: HostEventIterator<N.HostEvent, C.DiagnosticEvent> | undefined;
    stream = new HostEventIterator(nativeLane(this.#requireOpen().diagnostics()), (value) => {
      if (value.tag !== N.HostEvent_Tags.Diagnostic) throw new Error("Unexpected application event in diagnostic lane");
      return A.liftDiagnosticEvent(value.inner.event);
    }, options.signal, () => { if (stream !== undefined) this.#streams.delete(stream); });
    if (!stream.closed) this.#streams.add(stream);
    return stream;
  }
  beginResourceUpload(linkId: C.LinkId, totalBytes: bigint, options: {
    metadata?: Uint8Array; compression?: C.ResourceCompression;
  } = {}): ResourceUpload {
    const upload = new ResourceUpload(this.#requireOpen().beginResourceUpload(
      linkId, totalBytes, options.metadata, A.lowerResourceCompression(options.compression ?? { tag: "Auto", data: undefined }),
    ), () => { this.#streams.delete(upload); });
    this.#streams.add(upload);
    return upload;
  }
  release(): void {
    if (this.#released) return;
    this.#released = true;
    for (const stream of this.#streams) stream.close();
    this.#streams.clear();
    this.#native.uniffiDestroy();
  }
}

/** Only a caller that opens a session receives shutdown authority. */
export class OwnedHostSession {
  readonly host: HostClient;
  readonly #native: N.HostSession;
  #stop: Promise<void> | undefined;
  constructor(session: N.HostSessionLike) {
    if (!(session instanceof N.HostSession)) throw new TypeError("Expected a generated PRNS HostSession object");
    this.#native = session;
    this.host = new HostClient(session.client());
  }
  stop(): Promise<void> {
    this.#stop ??= this.#native.stop().then(() => {
      this.host.release();
      this.#native.uniffiDestroy();
    }, (error: unknown) => {
      if (error instanceof N.BindingError.Backend) {
        // The stop binding reports Backend only after native shutdown has joined.
        // Keep its settled failure, but release leases just as on successful stop.
        this.host.release();
        this.#native.uniffiDestroy();
      } else {
        // Unavailable ownership or a transport failure does not prove completion.
        this.#stop = undefined;
      }
      throw error;
    });
    return this.#stop;
  }
  close(): Promise<void> { return this.stop(); }
}

export async function openHost(config: C.HostConfig, options: { remoteControl?: R.RemoteControlNativeRemoteControlConfig } = {}): Promise<OwnedHostSession> {
  const hostConfig = A.lowerHostConfig(config);
  return new OwnedHostSession(await (options.remoteControl === undefined
    ? N.openHost(hostConfig)
    : N.openHostWithRemoteControl(hostConfig, RA.lowerRemoteControlNativeRemoteControlConfig(options.remoteControl))));
}

/** Native extensions return the shared SDK's Host object through external types. */
export function borrowHost(host: N.HostClientHandleLike): HostClient { return new HostClient(host); }
export function backendInfo(): C.BackendInfo { return A.liftBackendInfo(N.backendInfo()); }

class ReceivedResource implements C.ResourceStream {
  readonly totalBytes: bigint;
  readonly #native: N.ResourceReader;
  #claimed = false;
  #closed = false;
  constructor(reader: N.ResourceReaderLike, totalBytes: bigint) {
    if (!(reader instanceof N.ResourceReader)) throw new TypeError("Expected a generated PRNS ResourceReader object");
    this.#native = reader;
    this.totalBytes = totalBytes;
  }
  claim(): ReturnType<C.ResourceStream["claim"]> {
    if (this.#claimed || this.#closed) return { tag: "AlreadyClaimed", data: { lane: "Resource" } };
    this.#claimed = true;
    const resource = this;
    return { tag: "Claimed", data: {
      [Symbol.asyncIterator]() { return this; },
      async next() {
        if (resource.#closed) return { done: true, value: undefined };
        const bytes = resource.#native.readChunk(64 * 1024);
        if (bytes === undefined) {
          resource.close();
          return { done: true, value: undefined };
        }
        return { done: false, value: bytes };
      },
      async return() { resource.close(); return { done: true, value: undefined }; },
    } };
  }
  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#native.closeReader();
    this.#native.uniffiDestroy();
  }
}

export class ResourceUpload {
  readonly #native: N.ResourceUpload;
  readonly #onClose: (() => void) | undefined;
  #closed = false;
  constructor(upload: N.ResourceUploadLike, onClose?: () => void) {
    if (!(upload instanceof N.ResourceUpload)) throw new TypeError("Expected a generated PRNS ResourceUpload object");
    this.#native = upload;
    this.#onClose = onClose;
  }
  writeChunk(bytes: Uint8Array, options?: { signal: AbortSignal }): Promise<void> {
    if (this.#closed) return Promise.reject(new Error("Resource upload closed"));
    return this.#native.writeChunk(bytes, options);
  }
  async finish(options?: { signal: AbortSignal }): Promise<C.CommandSettlement> {
    if (this.#closed) throw new Error("Resource upload closed");
    const settlement = A.liftCommandSettlement(await this.#native.finish(options));
    this.#closed = true;
    this.#native.uniffiDestroy();
    this.#onClose?.();
    return settlement;
  }
  abort(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#native.abort();
    this.#native.uniffiDestroy();
    this.#onClose?.();
  }
  close(): void { this.abort(); }
}

function nativeLane(stream: N.EventStreamLike) {
  if (!(stream instanceof N.EventStream)) throw new TypeError("Expected a generated PRNS EventStream object");
  return {
    ready: (options?: { signal: AbortSignal }) => stream.ready(options),
    tryNext: () => stream.tryNext(),
    close: () => { stream.closeStream(); stream.uniffiDestroy(); },
  };
}
