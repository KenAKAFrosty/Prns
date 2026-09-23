// Generated-object test doubles keep the production JS ownership wrapper intact.
export const BindingError = {
  Backend: class Backend extends Error {},
  OwnershipUnavailable: class OwnershipUnavailable extends Error {},
};

export class EventStream {
  closed = 0;
  destroyed = 0;
  closeStream() { this.closed++; }
  uniffiDestroy() { this.destroyed++; }
}

export class ResourceUpload {
  aborted = 0;
  destroyed = 0;
  abort() { this.aborted++; }
  uniffiDestroy() { this.destroyed++; }
}

export class HostClientHandle {
  destroyed = 0;
  stream = new EventStream();
  upload = new ResourceUpload();
  applicationEvents() { return this.stream; }
  beginResourceUpload() { return this.upload; }
  lifecycle() { return { state: 'Running' }; }
  uniffiDestroy() { this.destroyed++; }
}

export class HostSession {
  destroyed = 0;
  stops = 0;
  handle = new HostClientHandle();
  constructor(stop) { this.onStop = stop; }
  client() { return this.handle; }
  stop() { this.stops++; return this.onStop(); }
  uniffiDestroy() { this.destroyed++; }
}
