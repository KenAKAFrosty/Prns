import { FlashBridgeError } from "./core.js";
import { createEnrollmentWait, ENROLLMENT_POLL_MILLIS } from "./enrollment-wait.js";

// USB-Auto v2 local controls. Golden vectors are shared with prns-core.
const HELLO = Uint8Array.of(1, 0x50, 0x72, 0x6e, 0x73, 2, 0);
const CAPABILITY = 2;
const MAX_FRAME = 256;
const MAX_CHUNK = 4096;
const MAX_RECEIVED = 65536;
const MAX_QUEUED = 16;
const session = { operation: null };
let pendingCleanup = null;

export function espEnrollmentCleanup() { return pendingCleanup; }

export function frameUsbAuto(payload) {
  const bytes = [0x7e];
  for (const byte of payload) {
    if (byte === 0x7e || byte === 0x7d) bytes.push(0x7d, byte ^ 0x20);
    else bytes.push(byte);
  }
  bytes.push(0x7e);
  return Uint8Array.from(bytes);
}

function error(message) { return new FlashBridgeError("connection_failure", message); }
function unknown(message) { return { status: "unknown", message }; }
function hex(bytes) { return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join(""); }
function unhex(text) { return Uint8Array.from(text.match(/../g), (pair) => Number.parseInt(pair, 16)); }
function word(bytes, at = 0) { return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(at, true); }
function putWord(bytes, at, value) { new DataView(bytes.buffer).setUint32(at, value, true); }

function randomWord(environment, excluded = []) {
  const bytes = new Uint8Array(4);
  environment.crypto.getRandomValues(bytes);
  let value = word(bytes);
  while (value === 0 || excluded.includes(value)) value = (value + 1) >>> 0;
  return value;
}

class SerialFrames {
  constructor(reader, dependencies) {
    this.reader = reader;
    this.dependencies = dependencies;
    this.buffer = [];
    this.frames = [];
    this.escaped = false;
    this.discard = false;
    this.received = 0;
    this.pendingRead = null;
  }

  push(chunk) {
    if (!(chunk instanceof Uint8Array) || chunk.length > MAX_CHUNK
        || (this.received += chunk.length) > MAX_RECEIVED) {
      throw error("The serial input exceeded the setup safety limit.");
    }
    for (const byte of chunk) {
      if (byte === 0x7e) {
        if (!this.discard && this.buffer.length) {
          if (this.frames.length >= MAX_QUEUED) throw error("Too many serial setup frames arrived.");
          this.frames.push(Uint8Array.from(this.buffer));
        }
        this.buffer.length = 0;
        this.discard = false;
        this.escaped = false;
      } else if (!this.discard) {
        if (byte === 0x7d) { this.escaped = true; continue; }
        this.buffer.push(this.escaped && (byte === 0x5e || byte === 0x5d) ? byte ^ 0x20 : byte);
        this.escaped = false;
        if (this.buffer.length > MAX_FRAME) {
          this.buffer.length = 0;
          this.discard = true;
        }
      }
    }
  }

  async next(milliseconds) {
    if (this.frames.length) return this.frames.shift();
    const now = this.dependencies.now ?? (() => performance.now());
    const deadline = now() + milliseconds;
    // A timed read keeps ownership of the same pending reader operation. A retry
    // must not leave competing reads that can consume a later acknowledgement.
    while (!this.frames.length) {
      this.pendingRead ??= this.reader.read();
      if (now() >= deadline) throw Object.assign(error("The serial setup response timed out."), { setupTimeout: true });
      const result = await bounded(this.pendingRead, Math.max(1, deadline - now()), this.dependencies);
      this.pendingRead = null;
      if (result.done) throw error("The board’s serial connection ended.");
      this.push(result.value);
    }
    return this.frames.shift();
  }
}

function bounded(promise, milliseconds, dependencies) {
  let timer;
  const set = dependencies.setTimeoutImpl ?? globalThis.setTimeout;
  const clear = dependencies.clearTimeoutImpl ?? globalThis.clearTimeout;
  const timeout = new Promise((_, reject) => {
    timer = set(() => reject(Object.assign(error("The serial setup response timed out."), { setupTimeout: true })), milliseconds);
  });
  return Promise.race([promise, timeout]).finally(() => clear(timer));
}

function statusFrame(frame) {
  if (frame[0] !== 6) return null;
  if (frame.length !== 74 || frame[5] > 3 || (frame[5] === 0 && word(frame, 6) !== 0)) throw error("The board returned a malformed setup receipt.");
  return { nonce: word(frame, 1), status: frame[5], transaction: word(frame, 6), targetPublicKey: hex(frame.subarray(10)) };
}

async function matchingStatus(frames, nonce, deadline, now, transaction) {
  while (now() < deadline) {
    const receipt = statusFrame(await frames.next(Math.max(1, deadline - now())));
    if (receipt?.nonce === nonce && (transaction === undefined || receipt.transaction === transaction)) {
      // Bound noise per correlated exchange, not the lifetime of a connection
      // that the user may explicitly keep open through multiple wait windows.
      frames.received = 0;
      return receipt;
    }
  }
  throw error("The board did not return a matching setup receipt.");
}

export async function requestEspControllerEnrollment(serial, request, environment, dependencies = {}) {
  if (!/^[0-9a-f]{128}$/i.test(request.controllerPublicKey ?? "")
      || !["authorize", "check-status"].includes(request.action)
      || request.transport !== "esp-serial" || typeof request.boardSlug !== "string" || !request.boardSlug.length) {
    throw new FlashBridgeError("invalid_request", "Enter a valid controller public card and select a board.");
  }
  if (pendingCleanup) throw error("The previous serial connection has not closed. Close this page before using the board again.");
  const retained = dependencies.enrollmentSession ?? session;
  const identity = `${request.boardSlug}:${request.controllerPublicKey.toLowerCase()}`;
  let operation = request.action === "check-status" ? retained.operation : null;
  if (request.action === "check-status" && (!operation || operation.identity !== identity)) {
    return unknown("No setup attempt for this controller and board is retained on this page. Check access from your controller before starting a new authorization.");
  }
  const now = dependencies.now ?? (() => performance.now());
  const sleep = dependencies.sleep ?? ((ms) => new Promise((resolve) => setTimeout(resolve, ms)));
  const responseTimeout = dependencies.responseTimeoutMs ?? 1500;
  const handshakeTimeout = dependencies.handshakeTimeoutMs ?? 12000;
  const wait = dependencies.enrollmentWait ?? createEnrollmentWait(dependencies);
  let port, reader, writer, openAttempt, pendingWrite, opened = false, outcome;
  try {
    // USB bridge VID/PID is not a board identity. The user selects the port; the
    // running protocol capability and full target key gate all authorization.
    port = await serial.requestPort({ filters: [] });
    wait.throwIfStopped();
    if (port.readable || port.writable) throw error("This serial port is already open. Close the other connection first.");
    openAttempt = Promise.resolve().then(() => port.open({ baudRate: 115200, dataBits: 8, stopBits: 1, parity: "none", flowControl: "none", bufferSize: 2048 })).then(() => { opened = true; });
    await bounded(openAttempt, responseTimeout, dependencies);
    wait.throwIfStopped();
    reader = port.readable?.getReader();
    writer = port.writable?.getWriter();
    if (!reader || !writer) throw error("The board’s serial streams are unavailable.");
    const frames = new SerialFrames(reader, dependencies);
    const write = (payload) => {
      pendingWrite = writer.write(frameUsbAuto(payload));
      return bounded(pendingWrite, responseTimeout, dependencies);
    };
    const deadline = now() + handshakeTimeout;
    let managed = false;
    while (!managed && now() < deadline) {
      await write(HELLO);
      const probeDeadline = Math.min(deadline, now() + 300);
      try {
        while (now() < probeDeadline) {
          const frame = await frames.next(Math.max(1, probeDeadline - now()));
          if (frame[0] !== 2) continue;
          if (frame.length !== 15 || !HELLO.subarray(1, 6).every((byte, index) => frame[index + 1] === byte)) {
            throw error("This serial device did not identify compatible running Hopspot firmware.");
          }
          if (!(frame[6] & CAPABILITY)) throw error("This firmware does not support serial controller setup. Install a compatible build first.");
          managed = true;
          break;
        }
      } catch (failure) {
        if (!failure.setupTimeout) throw failure;
      }
    }
    if (!managed) throw error("No compatible running Hopspot firmware answered. Check the selected port and let the board finish starting.");
    wait.throwIfStopped();
    let nonce = retained.nonce ?? randomWord(environment);
    const query = async () => {
      nonce = ((nonce + 1) >>> 0) || 1;
      retained.nonce = nonce;
      const payload = new Uint8Array(5);
      payload[0] = 5;
      putWord(payload, 1, nonce);
      // Hello maintains the running serial interface's protocol presence.
      await write(HELLO);
      await write(payload);
      return matchingStatus(frames, nonce, now() + responseTimeout, now);
    };
    const before = await query();
    wait.throwIfStopped();
    let receipt = before;
    if (request.action === "authorize") {
      if (before.status === 1) throw error("The board is still saving a setup request. Check its status before starting another authorization.");
      const transaction = randomWord(environment, [before.transaction, retained.operation?.transaction]);
      const payload = new Uint8Array(133);
      payload[0] = 4;
      putWord(payload, 1, transaction);
      payload.set(unhex(request.controllerPublicKey), 5);
      payload.set(unhex(before.targetPublicKey), 69);
      operation = { identity, transaction, targetPublicKey: before.targetPublicKey, submitted: true };
      retained.operation = operation;
      await write(payload); // Exactly one authorization; a lost acknowledgement is unknown.
      receipt = await matchingStatus(frames, 0, now() + responseTimeout, now, transaction);
    }
    while (true) {
      if (receipt.targetPublicKey !== operation.targetPublicKey) {
        outcome = unknown("This board has a different identity. Reconnect the board used for this setup and check its status before starting a new authorization.");
        break;
      }
      if (receipt.transaction !== operation.transaction || receipt.status === 0) {
        outcome = unknown("The board has no matching setup receipt. It may have restarted or handled another request. Check access from your controller before explicitly authorizing again.");
        break;
      }
      if (receipt.status === 2) {
        outcome = { status: "saved", targetPublicKey: receipt.targetPublicKey };
        break;
      }
      if (receipt.status === 3) {
        operation.submitted = false;
        retained.operation = null;
        throw error("The controller authorization was rejected or could not be saved.");
      }
      outcome = await wait.afterPending();
      if (outcome) break;
      await sleep(ENROLLMENT_POLL_MILLIS);
      receipt = await query();
    }
  } catch (failure) {
    outcome = operation?.submitted
      ? unknown("Controller setup could not be confirmed. Reconnect and check setup status; this does not authorize the controller again.")
      : { status: "error", message: failure?.message ?? "Could not open the board’s serial connection." };
  } finally {
    wait.close();
    if (openAttempt) {
      const cleanup = (async () => {
        // A timed-out open/write is still owned until it settles. Do not let
        // flashing or another setup claim the port while that work can resume.
        await openAttempt.catch(() => {});
        if (!opened) return;
        try { await reader?.cancel(); } catch {}
        try { reader?.releaseLock(); } catch {}
        await pendingWrite?.catch(() => {});
        try { writer?.releaseLock(); } catch {}
        // A successful close confirms ownership ended even if cancellation saw
        // an already-disconnected stream. Failed close keeps the operation locked.
        await port.close();
      })();
      pendingCleanup = cleanup;
      cleanup.then(() => { if (pendingCleanup === cleanup) pendingCleanup = null; }, () => {});
      try { await bounded(cleanup, responseTimeout, dependencies); }
      catch {
        outcome = unknown("The serial connection could not be released. Setup may have completed; close this page before reconnecting and checking access.");
      }
    }
  }
  return outcome;
}
