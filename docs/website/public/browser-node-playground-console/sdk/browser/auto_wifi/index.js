import { Tag, match_into } from "../../casework.js";
import { RecoverySchedule, } from "./recovery.js";
const RENDEZVOUS_PORT = 42_721;
const RENDEZVOUS_PATH = "/prns";
const CATALOG_PATH = "/.well-known/prns-transport";
const SUBPROTOCOL = "prns.transport.v1";
const PROTOCOL_VERSION = 1;
const ID_HEX_LENGTH = 32;
const ID_BYTE_LENGTH = 16;
const CLIENT_HELLO_LENGTH = 10;
const SERVER_HELLO_LENGTH = 26;
const MAX_GATEWAYS = 3;
const MAX_CATALOG_GATEWAYS = 64;
const MAX_CATALOG_BODY_BYTES = 16 * 1024;
const CONNECT_TIMEOUT_MS = 10_000;
const FETCH_TIMEOUT_MS = 10_000;
const REFRESH_INTERVAL_MS = 15_000;
const CATALOG_EXPIRY_MS = 60_000;
const OUTBOUND_POLL_MS = 25;
const BUFFER_POLL_MS = 4;
const MIN_BUFFER_LIMIT = 1024 * 1024;
const MAX_PENDING_TRANSPORT_FRAMES = 64;
const WEBSOCKET_OPEN = 1;
const SELECTION_SEED_KEY = "prns.browser-gateway-selection-seed.v1";
const SELECTION_DOMAIN = new TextEncoder().encode("prns browser gateway selection v1");
const HELLO_MAGIC = new Uint8Array([
    0x50, 0x52, 0x4e, 0x53, 0x57, 0x53, 0x00, 0x00,
]);
const DIRECT_URLS = [
    `ws://localhost:${RENDEZVOUS_PORT}${RENDEZVOUS_PATH}`,
    `ws://prns.local:${RENDEZVOUS_PORT}${RENDEZVOUS_PATH}`,
];
const CATALOG_PROBES = [
    {
        url: `http://localhost:${RENDEZVOUS_PORT}${CATALOG_PATH}`,
        targetAddressSpace: "loopback",
    },
    {
        url: `http://prns.local:${RENDEZVOUS_PORT}${CATALOG_PATH}`,
        targetAddressSpace: "local",
    },
];
export class AutoWifiInterface {
    name = "auto-wifi";
    #host;
    #controller;
    constructor(host) {
        this.#host = host;
    }
    start() {
        if (this.#controller && !this.#controller.closed) {
            return this.#controller;
        }
        this.#controller = new AutoWifiController(this.#host);
        return this.#controller;
    }
}
export class AutoWifiController {
    #host;
    #known = new Map();
    #sessions = new Map();
    #recoveries = new RecoverySchedule();
    #seed;
    #status = Tag("Starting");
    #refreshing = false;
    #refreshPending = false;
    #closed = false;
    #attempt = 0;
    #refreshTimer;
    #recoveryTimer;
    constructor(host) {
        this.#host = host;
        this.#seed = loadSelectionSeed();
        this.#refreshTimer = globalThis.setInterval(() => {
            this.#requestRefresh();
        }, REFRESH_INTERVAL_MS);
        this.#requestRefresh();
    }
    get status() {
        return this.#status;
    }
    get closed() {
        return this.#closed;
    }
    async close() {
        if (this.#closed) {
            return Tag("Closed");
        }
        this.#closed = true;
        if (this.#refreshTimer !== undefined) {
            globalThis.clearInterval(this.#refreshTimer);
        }
        if (this.#recoveryTimer !== undefined) {
            globalThis.clearTimeout(this.#recoveryTimer);
        }
        const sessions = [...this.#sessions.values()];
        this.#sessions.clear();
        this.#known.clear();
        this.#recoveries.clear();
        const outcomes = await Promise.all(sessions.map((session) => session.close()));
        this.#status = Tag("Closed");
        return (outcomes.find((outcome) => outcome.tag === "RuntimeRejected") ?? Tag("Closed"));
    }
    #requestRefresh() {
        if (this.#closed) {
            return;
        }
        if (this.#refreshing) {
            this.#refreshPending = true;
            return;
        }
        void this.#refresh();
    }
    async #refresh() {
        if (this.#closed) {
            return;
        }
        this.#refreshing = true;
        const dueRecoveries = this.#recoveries.due(Date.now());
        const dueRecoveryIds = new Set(dueRecoveries.map((recovery) => recovery.key));
        this.#attempt += 1;
        if (this.#sessions.size === 0) {
            this.#status = Tag("Discovering", { attempt: this.#attempt });
        }
        try {
            const ready = this.#host.autoWifiReady();
            if (ready.tag !== "Ready") {
                this.#setUnavailable(ready);
                return;
            }
            const seed = await this.#seed;
            if (seed.tag !== "Loaded") {
                this.#setUnavailable(seed);
                return;
            }
            const frameCap = this.#host.autoWifiFrameCap();
            const directPromise = Promise.all(DIRECT_URLS.map((url) => this.#mayProbeUrl(url, dueRecoveryIds)
                ? probeGateway(url, frameCap)
                : Promise.resolve(Tag("Skipped"))));
            const catalogPromise = Promise.all(CATALOG_PROBES.map((probe) => fetchCatalog(probe)));
            const [direct, catalogs] = await Promise.all([
                directPromise,
                catalogPromise,
            ]);
            if (this.#closed) {
                closeProbes(direct);
                return;
            }
            const failures = [];
            const now = Date.now();
            for (const outcome of catalogs) {
                if (outcome.tag === "Failed") {
                    failures.push(outcome.data);
                    continue;
                }
                for (const gateway of outcome.data) {
                    this.#known.set(gateway.id, {
                        ...gateway,
                        lastSeen: now,
                        localhost: isLocalhostUrl(gateway.url),
                    });
                }
            }
            for (const [id, gateway] of this.#known) {
                if (now - gateway.lastSeen > CATALOG_EXPIRY_MS &&
                    !this.#recoveries.has(id)) {
                    this.#known.delete(id);
                }
            }
            const probes = new Map();
            for (const outcome of direct) {
                if (outcome.tag === "Skipped") {
                    continue;
                }
                if (outcome.tag === "Failed") {
                    failures.push(outcome.data);
                    continue;
                }
                const existing = probes.get(outcome.data.id);
                if (!existing || (!existing.localhost && outcome.data.localhost)) {
                    existing?.pending.close();
                    probes.set(outcome.data.id, outcome.data);
                }
                else {
                    outcome.data.pending.close();
                }
            }
            const candidates = new Map();
            for (const [id, session] of this.#sessions) {
                candidates.set(id, {
                    id,
                    url: session.url,
                    localhost: session.localhost,
                    session,
                });
            }
            for (const [id, gateway] of this.#known) {
                if (!candidates.has(id) &&
                    this.#mayAttemptGateway(id, dueRecoveryIds)) {
                    candidates.set(id, { ...gateway });
                }
            }
            for (const [id, probe] of probes) {
                const existing = candidates.get(id);
                if (!this.#mayAttemptGateway(id, dueRecoveryIds)) {
                    probe.pending.close();
                    continue;
                }
                if (existing?.session) {
                    probe.pending.close();
                    continue;
                }
                candidates.set(id, {
                    id,
                    url: probe.url,
                    localhost: probe.localhost,
                    probe,
                });
            }
            const weighted = await Promise.all([...candidates.values()].map(async (candidate) => {
                const outcome = await gatewayWeight(seed.data, candidate.id);
                return outcome.tag === "Weighted"
                    ? Tag("Ranked", { ...candidate, weight: outcome.data })
                    : outcome;
            }));
            const ranked = [];
            for (const outcome of weighted) {
                if (outcome.tag === "SelectionIdentityUnavailable") {
                    for (const probe of probes.values()) {
                        probe.pending.close();
                    }
                    this.#setUnavailable(outcome);
                    return;
                }
                ranked.push(outcome.data);
            }
            ranked.sort(compareCandidates);
            const selected = new Set();
            for (const candidate of ranked) {
                if (selected.size >= MAX_GATEWAYS || this.#closed) {
                    break;
                }
                const existing = this.#sessions.get(candidate.id);
                if (existing) {
                    selected.add(candidate.id);
                    continue;
                }
                let probe = candidate.probe;
                if (!probe) {
                    const outcome = await probeGateway(candidate.url, frameCap);
                    if (outcome.tag === "Failed") {
                        failures.push(outcome.data);
                        continue;
                    }
                    probe = outcome.data;
                    if (probe.id !== candidate.id) {
                        probe.pending.close();
                        failures.push(Tag("DiscoveryFailed", {
                            detail: `catalog ID ${candidate.id} did not match gateway hello ${probe.id}`,
                        }));
                        continue;
                    }
                }
                const attached = this.#attach(probe);
                if (attached.tag === "Attached") {
                    selected.add(candidate.id);
                }
                else {
                    failures.push(attached);
                }
            }
            for (const [id, session] of this.#sessions) {
                if (!selected.has(id)) {
                    this.#sessions.delete(id);
                    const detached = await session.close();
                    if (detached.tag === "RuntimeRejected") {
                        failures.push(detached);
                    }
                }
            }
            for (const [id, probe] of probes) {
                if (!this.#sessions.has(id)) {
                    probe.pending.close();
                }
            }
            if (this.#sessions.size > 0) {
                this.#publishActive();
            }
            else {
                this.#setUnavailable(preferredFailure(failures));
            }
        }
        catch (error) {
            this.#setUnavailable(Tag("DiscoveryFailed", { detail: describeError(error) }));
        }
        finally {
            this.#retryUnrecovered(dueRecoveries);
            this.#refreshing = false;
            if (this.#refreshPending) {
                this.#refreshPending = false;
                this.#requestRefresh();
            }
            else {
                this.#scheduleRecovery();
            }
        }
    }
    #attach(probe) {
        if (this.#sessions.has(probe.id) || this.#sessions.size >= MAX_GATEWAYS) {
            probe.pending.close();
            return Tag("DiscoveryFailed", {
                detail: `gateway ${probe.id} cannot attach outside the selected set`,
            });
        }
        const claimed = probe.pending.claim();
        if (claimed.tag !== "Claimed") {
            return Tag("DiscoveryFailed", {
                detail: `gateway ${probe.id} disconnected before runtime registration`,
            });
        }
        const registered = this.#host.autoWifiRegister(probe.idBytes);
        if (registered.tag !== "Registered") {
            closeSocket(claimed.data.socket);
            return registered;
        }
        this.#retireReplacedGateways(probe);
        this.#known.set(probe.id, {
            id: probe.id,
            url: probe.url,
            localhost: probe.localhost,
            lastSeen: Date.now(),
        });
        this.#recoveries.complete(probe.id);
        let session;
        session = new AutoWifiGatewaySession(this.#host, claimed.data.socket, claimed.data.frames, registered.data, probe.id, probe.url, probe.localhost, (outcome) => this.#sessionClosed(probe.id, session, outcome));
        this.#sessions.set(probe.id, session);
        session.start();
        return Tag("Attached");
    }
    #sessionClosed(id, session, outcome) {
        if (this.#sessions.get(id) !== session) {
            return;
        }
        this.#sessions.delete(id);
        if (this.#closed) {
            return;
        }
        const now = Date.now();
        this.#known.set(id, {
            id,
            url: session.url,
            localhost: session.localhost,
            lastSeen: now,
        });
        this.#recoveries.begin(id, now);
        if (this.#sessions.size > 0) {
            this.#publishActive();
        }
        else if (outcome.tag === "RuntimeRejected") {
            this.#setUnavailable(outcome);
        }
        else {
            this.#status = Tag("Discovering", { attempt: this.#attempt + 1 });
        }
        this.#scheduleRecovery();
        this.#requestRefresh();
    }
    #publishActive() {
        const gateways = [...this.#sessions.values()]
            .map((session) => session.snapshot())
            .sort((left, right) => {
            if (left.localhost !== right.localhost) {
                return left.localhost ? -1 : 1;
            }
            return left.id.localeCompare(right.id);
        });
        this.#status = Tag("Active", { gateways });
    }
    #setUnavailable(failure) {
        if (!this.#closed && this.#sessions.size === 0) {
            this.#status = Tag("Unavailable", failure);
        }
    }
    #mayProbeUrl(url, dueRecoveryIds) {
        if ([...this.#sessions.values()].some((session) => session.url === url)) {
            return false;
        }
        return ![...this.#known.values()].some((gateway) => gateway.url === url &&
            !this.#mayAttemptGateway(gateway.id, dueRecoveryIds));
    }
    #mayAttemptGateway(id, dueRecoveryIds) {
        return !this.#recoveries.has(id) || dueRecoveryIds.has(id);
    }
    #retireReplacedGateways(probe) {
        for (const [id, gateway] of this.#known) {
            if (id !== probe.id && gateway.url === probe.url) {
                this.#known.delete(id);
                this.#recoveries.complete(id);
            }
        }
    }
    #retryUnrecovered(dueRecoveries) {
        const now = Date.now();
        for (const recovery of dueRecoveries) {
            if (!this.#sessions.has(recovery.key)) {
                this.#recoveries.retry(recovery, now);
            }
        }
    }
    #scheduleRecovery() {
        if (this.#recoveryTimer !== undefined) {
            globalThis.clearTimeout(this.#recoveryTimer);
            this.#recoveryTimer = undefined;
        }
        if (this.#closed) {
            return;
        }
        const dueAt = this.#recoveries.nextDueAt();
        if (dueAt === undefined) {
            return;
        }
        this.#recoveryTimer = globalThis.setTimeout(() => {
            this.#recoveryTimer = undefined;
            this.#requestRefresh();
        }, Math.max(0, dueAt - Date.now()));
    }
}
class PendingGatewaySocket {
    #socket;
    #frameCap;
    #frames = [];
    #bufferedBytes = 0;
    #closed = false;
    #claimed = false;
    constructor(socket, frameCap) {
        this.#socket = socket;
        this.#frameCap = frameCap;
        this.#socket.addEventListener("message", this.#handleMessage);
        this.#socket.addEventListener("close", this.#handleClose);
        this.#socket.addEventListener("error", this.#handleClose);
        if (!Number.isSafeInteger(frameCap) || frameCap <= 0) {
            this.close();
        }
    }
    claim() {
        if (this.#closed ||
            this.#claimed ||
            this.#socket.readyState !== WEBSOCKET_OPEN) {
            this.close();
            return Tag("Unavailable");
        }
        this.#claimed = true;
        this.#detach();
        return Tag("Claimed", {
            socket: this.#socket,
            frames: this.#frames.splice(0),
        });
    }
    close() {
        if (!this.#closed) {
            this.#closed = true;
            this.#detach();
        }
        closeSocket(this.#socket);
    }
    #handleMessage = (event) => {
        const measured = websocketPayloadLength(event.data);
        if (measured.tag !== "Measured") {
            this.close();
            return;
        }
        const length = measured.data;
        if (length > this.#frameCap ||
            this.#frames.length >= MAX_PENDING_TRANSPORT_FRAMES ||
            this.#bufferedBytes + length >
                this.#frameCap * MAX_PENDING_TRANSPORT_FRAMES) {
            this.close();
            return;
        }
        this.#frames.push(event.data);
        this.#bufferedBytes += length;
    };
    #handleClose = () => {
        this.#closed = true;
        this.#detach();
    };
    #detach() {
        this.#socket.removeEventListener("message", this.#handleMessage);
        this.#socket.removeEventListener("close", this.#handleClose);
        this.#socket.removeEventListener("error", this.#handleClose);
    }
}
class AutoWifiGatewaySession {
    #host;
    #socket;
    #frameCap;
    #bufferLimit;
    #initialFrames;
    #onClosed;
    interfaceId;
    id;
    url;
    localhost;
    #lifecycle = Tag("Open");
    #released = false;
    #writeQueue = Promise.resolve();
    #readQueue = Promise.resolve();
    constructor(host, socket, initialFrames, interfaceId, id, url, localhost, onClosed) {
        this.#host = host;
        this.#socket = socket;
        this.#initialFrames = initialFrames;
        this.#frameCap = host.autoWifiFrameCap();
        this.#bufferLimit = Math.max(MIN_BUFFER_LIMIT, this.#frameCap * 2);
        this.#onClosed = onClosed;
        this.interfaceId = interfaceId;
        this.id = id;
        this.url = url;
        this.localhost = localhost;
    }
    start() {
        this.#socket.addEventListener("message", (event) => {
            this.#queueInbound(event.data);
        });
        this.#socket.addEventListener("close", () => {
            void this.close();
        });
        this.#socket.addEventListener("error", () => {
            void this.close();
        });
        for (const frame of this.#initialFrames) {
            this.#queueInbound(frame);
        }
        void this.#outboundLoop().catch(async () => {
            await this.close();
        });
    }
    snapshot() {
        return {
            id: this.id,
            url: this.url,
            interfaceId: this.interfaceId,
            localhost: this.localhost,
        };
    }
    get #closed() {
        return this.#lifecycle.tag === "Closed";
    }
    async close() {
        if (this.#lifecycle.tag === "Closed") {
            return this.#lifecycle.data;
        }
        const detached = this.#host.autoWifiDeactivate(this.interfaceId);
        this.#lifecycle = Tag("Closed", detached);
        try {
            this.#socket.close();
        }
        catch {
            this.#release(detached);
            return detached;
        }
        await this.#writeQueue.catch(() => undefined);
        this.#release(detached);
        return detached;
    }
    #queueInbound(value) {
        this.#readQueue = this.#readQueue
            .then(async () => {
            const decoded = await websocketBytes(value, this.#frameCap);
            if (decoded.tag !== "Decoded") {
                await this.close();
                return;
            }
            const bytes = decoded.data;
            if (this.#closed || bytes.length === 0) {
                return;
            }
            const ingested = this.#host.autoWifiIngest(this.interfaceId, bytes);
            if (ingested.tag !== "Accepted") {
                await this.close();
            }
        })
            .catch(async () => {
            await this.close();
        });
    }
    async #outboundLoop() {
        while (!this.#closed) {
            const outbound = this.#host.autoWifiTakeOutbound(this.interfaceId);
            if (outbound.tag !== "Outbound") {
                await this.close();
                return;
            }
            for (const frame of outbound.data) {
                if (frame.bytes.length === 0 || frame.bytes.length > this.#frameCap) {
                    await this.close();
                    return;
                }
                this.#writeQueue = this.#writeQueue.then(async () => {
                    while (!this.#closed &&
                        this.#socket.bufferedAmount > this.#bufferLimit) {
                        await wait(BUFFER_POLL_MS);
                    }
                    if (!this.#closed && this.#socket.readyState === 1) {
                        this.#socket.send(frame.bytes);
                    }
                });
                try {
                    await this.#writeQueue;
                }
                catch {
                    await this.close();
                    return;
                }
            }
            await wait(OUTBOUND_POLL_MS);
        }
    }
    #release(outcome) {
        if (!this.#released) {
            this.#released = true;
            this.#onClosed(outcome);
        }
    }
}
async function probeGateway(url, frameCap) {
    const canonical = validateGatewayUrl(url);
    if (canonical.tag !== "Valid") {
        return Tag("Failed", canonical.data);
    }
    const WebSocketCtor = globalThis.WebSocket;
    if (!WebSocketCtor) {
        return Tag("Failed", Tag("HostApiUnavailable", { api: "WebSocket" }));
    }
    let socket;
    try {
        socket = new WebSocketCtor(canonical.data, SUBPROTOCOL);
        socket.binaryType = "arraybuffer";
    }
    catch (error) {
        return Tag("Failed", browserNetworkFailure(error));
    }
    return new Promise((resolve) => {
        let phase = "opening";
        let settled = false;
        const timeout = globalThis.setTimeout(() => {
            settle(Tag("Failed", Tag("DiscoveryFailed", { detail: `gateway handshake timed out for ${canonical.data}` })));
        }, CONNECT_TIMEOUT_MS);
        const cleanup = () => {
            globalThis.clearTimeout(timeout);
            socket.removeEventListener("open", handleOpen);
            socket.removeEventListener("message", handleMessage);
            socket.removeEventListener("error", handleError);
            socket.removeEventListener("close", handleClose);
        };
        const settle = (outcome) => {
            if (settled) {
                return;
            }
            settled = true;
            cleanup();
            if (outcome.tag === "Failed") {
                closeSocket(socket);
            }
            resolve(outcome);
        };
        const handleOpen = () => {
            if (socket.protocol !== SUBPROTOCOL) {
                settle(Tag("Failed", Tag("DiscoveryFailed", {
                    detail: `gateway did not select ${SUBPROTOCOL}`,
                })));
                return;
            }
            phase = "hello";
            try {
                socket.send(clientHello());
            }
            catch (error) {
                settle(Tag("Failed", browserNetworkFailure(error)));
            }
        };
        const handleMessage = (event) => {
            if (phase !== "hello") {
                settle(Tag("Failed", Tag("DiscoveryFailed", {
                    detail: "gateway sent traffic before the bounded server hello completed",
                })));
                return;
            }
            phase = "decoding";
            void websocketBytes(event.data, SERVER_HELLO_LENGTH)
                .then((frame) => {
                if (frame.tag !== "Decoded") {
                    settle(Tag("Failed", Tag("DiscoveryFailed", {
                        detail: describeWebSocketDecodeFailure(frame),
                    })));
                    return;
                }
                const decoded = decodeServerHello(frame.data);
                if (decoded.tag !== "Decoded") {
                    settle(Tag("Failed", Tag("DiscoveryFailed", {
                        detail: decoded.data.detail,
                    })));
                    return;
                }
                settle(Tag("Connected", {
                    ...decoded.data,
                    url: canonical.data,
                    pending: new PendingGatewaySocket(socket, frameCap),
                    localhost: isLocalhostUrl(canonical.data),
                }));
            })
                .catch((error) => {
                settle(Tag("Failed", browserNetworkFailure(error)));
            });
        };
        const handleError = () => {
            settle(Tag("Failed", Tag("DiscoveryFailed", {
                detail: `gateway WebSocket failed for ${canonical.data}`,
            })));
        };
        const handleClose = () => {
            settle(Tag("Failed", Tag("DiscoveryFailed", {
                detail: `gateway WebSocket closed during ${phase}`,
            })));
        };
        socket.addEventListener("open", handleOpen);
        socket.addEventListener("message", handleMessage);
        socket.addEventListener("error", handleError);
        socket.addEventListener("close", handleClose);
    });
}
async function fetchCatalog(probe) {
    if (!globalThis.fetch) {
        return Tag("Failed", Tag("HostApiUnavailable", { api: "Fetch" }));
    }
    const controller = new AbortController();
    const timeout = globalThis.setTimeout(() => controller.abort(), FETCH_TIMEOUT_MS);
    try {
        const response = await globalThis.fetch(probe.url, {
            cache: "no-store",
            credentials: "omit",
            mode: "cors",
            redirect: "error",
            signal: controller.signal,
            targetAddressSpace: probe.targetAddressSpace,
        });
        if (!response.ok || response.redirected) {
            return Tag("Failed", Tag("DiscoveryFailed", {
                detail: `catalog ${probe.url} returned HTTP ${response.status}`,
            }));
        }
        const body = await readCappedBody(response);
        if (body.tag !== "Read") {
            return body;
        }
        return parseCatalog(body.data);
    }
    catch (error) {
        return Tag("Failed", browserNetworkFailure(error));
    }
    finally {
        globalThis.clearTimeout(timeout);
    }
}
async function readCappedBody(response) {
    const declared = response.headers.get("Content-Length");
    if (declared !== null) {
        const length = Number(declared);
        if (!Number.isSafeInteger(length) || length < 0 || length > MAX_CATALOG_BODY_BYTES) {
            return Tag("Failed", Tag("DiscoveryFailed", { detail: "catalog Content-Length exceeds the response cap" }));
        }
    }
    if (!response.body) {
        const bytes = new Uint8Array(await response.arrayBuffer());
        return bytes.length <= MAX_CATALOG_BODY_BYTES
            ? Tag("Read", bytes)
            : Tag("Failed", Tag("DiscoveryFailed", { detail: "catalog body exceeds the response cap" }));
    }
    const reader = response.body.getReader();
    const chunks = [];
    let length = 0;
    try {
        while (true) {
            const next = await reader.read();
            if (next.done) {
                break;
            }
            length += next.value.length;
            if (length > MAX_CATALOG_BODY_BYTES) {
                await reader.cancel();
                return Tag("Failed", Tag("DiscoveryFailed", { detail: "catalog body exceeds the response cap" }));
            }
            chunks.push(next.value);
        }
    }
    finally {
        reader.releaseLock();
    }
    const body = new Uint8Array(length);
    let offset = 0;
    for (const chunk of chunks) {
        body.set(chunk, offset);
        offset += chunk.length;
    }
    return Tag("Read", body);
}
export function parseBrowserGatewayCatalog(bytes) {
    return parseCatalog(bytes);
}
function parseCatalog(bytes) {
    if (bytes.length > MAX_CATALOG_BODY_BYTES) {
        return Tag("Failed", Tag("DiscoveryFailed", { detail: "catalog body exceeds the response cap" }));
    }
    let value;
    try {
        value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
    }
    catch (error) {
        return Tag("Failed", Tag("DiscoveryFailed", { detail: describeError(error) }));
    }
    if (!exactRecord(value, ["gateways", "version"]) || value.version !== 1) {
        return Tag("Failed", Tag("DiscoveryFailed", { detail: "catalog must be an exact version 1 object" }));
    }
    if (!Array.isArray(value.gateways) || value.gateways.length > MAX_CATALOG_GATEWAYS) {
        return Tag("Failed", Tag("DiscoveryFailed", { detail: "catalog gateways must be a bounded array" }));
    }
    const ids = new Set();
    const gateways = [];
    for (const raw of value.gateways) {
        if (!exactRecord(raw, ["id", "url"]) || typeof raw.id !== "string" || typeof raw.url !== "string") {
            return Tag("Failed", Tag("DiscoveryFailed", { detail: "catalog gateway has an invalid shape" }));
        }
        const id = parseRendezvousId(raw.id);
        const url = validateGatewayUrl(raw.url);
        if (id.tag !== "Parsed" || url.tag !== "Valid" || ids.has(id.data)) {
            return Tag("Failed", Tag("DiscoveryFailed", { detail: "catalog gateway ID or URL is invalid or duplicated" }));
        }
        ids.add(id.data);
        gateways.push({ id: id.data, url: url.data });
    }
    return Tag("Discovered", gateways);
}
export function validateBrowserGatewayUrl(value) {
    const validated = validateGatewayUrl(value);
    return validated.tag === "Valid" ? validated : Tag("Invalid");
}
function validateGatewayUrl(value) {
    let url;
    try {
        url = new URL(value);
    }
    catch (error) {
        return Tag("Invalid", Tag("DiscoveryFailed", { detail: describeError(error) }));
    }
    if (url.protocol !== "ws:" ||
        url.username !== "" ||
        url.password !== "" ||
        url.port !== RENDEZVOUS_PORT.toString() ||
        url.pathname !== RENDEZVOUS_PATH ||
        url.search !== "" ||
        url.hash !== "" ||
        !isPermittedHost(url.hostname)) {
        return Tag("Invalid", Tag("DiscoveryFailed", { detail: `invalid local gateway URL ${value}` }));
    }
    return Tag("Valid", url.toString());
}
function isPermittedHost(hostname) {
    const hostnameLower = hostname.toLowerCase();
    if (hostnameLower === "localhost") {
        return true;
    }
    if (isLocalHostname(hostnameLower)) {
        return true;
    }
    const unbracketed = hostnameLower.startsWith("[") && hostnameLower.endsWith("]")
        ? hostnameLower.slice(1, -1)
        : hostnameLower;
    if (unbracketed.includes(":")) {
        if (unbracketed === "::1") {
            return true;
        }
        const first = Number.parseInt(unbracketed.split(":", 1)[0] ?? "", 16);
        return (Number.isInteger(first) &&
            ((first & 0xfe00) === 0xfc00 || (first & 0xffc0) === 0xfe80));
    }
    const octets = unbracketed.split(".").map(Number);
    if (octets.length !== 4 ||
        octets.some((octet) => !Number.isInteger(octet) || octet < 0 || octet > 255)) {
        return false;
    }
    const [first, second] = octets;
    return (first === 127 ||
        first === 10 ||
        (first === 172 && second >= 16 && second <= 31) ||
        (first === 192 && second === 168) ||
        (first === 169 && second === 254));
}
function isLocalHostname(hostname) {
    if (hostname.length <= ".local".length ||
        hostname.length > 253 ||
        !hostname.endsWith(".local")) {
        return false;
    }
    return hostname.split(".").every((label) => label.length > 0 &&
        label.length <= 63 &&
        /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(label));
}
async function loadSelectionSeed() {
    try {
        const storage = globalThis.localStorage;
        if (!storage) {
            return Tag("SelectionIdentityUnavailable", { detail: "LocalStorage is unavailable" });
        }
        const stored = storage.getItem(SELECTION_SEED_KEY);
        if (stored !== null) {
            if (!/^[0-9a-f]{32}$/.test(stored)) {
                return Tag("SelectionIdentityUnavailable", { detail: "stored browser gateway selection seed is malformed" });
            }
            return Tag("Loaded", hexBytes(stored));
        }
        if (!globalThis.crypto) {
            return Tag("SelectionIdentityUnavailable", { detail: "Crypto is unavailable" });
        }
        const seed = new Uint8Array(ID_BYTE_LENGTH);
        globalThis.crypto.getRandomValues(seed);
        storage.setItem(SELECTION_SEED_KEY, bytesHex(seed));
        return Tag("Loaded", seed);
    }
    catch (error) {
        return Tag("SelectionIdentityUnavailable", { detail: describeError(error) });
    }
}
async function gatewayWeight(seed, id) {
    const subtle = globalThis.crypto?.subtle;
    if (!subtle) {
        return Tag("SelectionIdentityUnavailable", {
            detail: "SubtleCrypto is unavailable for stable gateway selection",
        });
    }
    const idBytes = hexBytes(id);
    const input = new Uint8Array(SELECTION_DOMAIN.length + seed.length + idBytes.length);
    input.set(SELECTION_DOMAIN, 0);
    input.set(seed, SELECTION_DOMAIN.length);
    input.set(idBytes, SELECTION_DOMAIN.length + seed.length);
    try {
        const digest = new Uint8Array(await subtle.digest("SHA-256", input));
        return Tag("Weighted", bytesHex(digest));
    }
    catch (error) {
        return Tag("SelectionIdentityUnavailable", {
            detail: `stable gateway selection: ${describeError(error)}`,
        });
    }
}
function compareCandidates(left, right) {
    if (left.localhost !== right.localhost) {
        return left.localhost ? -1 : 1;
    }
    const weightOrder = right.weight.localeCompare(left.weight);
    return weightOrder !== 0 ? weightOrder : left.id.localeCompare(right.id);
}
function clientHello() {
    const bytes = new Uint8Array(CLIENT_HELLO_LENGTH);
    bytes.set(HELLO_MAGIC);
    bytes[CLIENT_HELLO_LENGTH - 2] = PROTOCOL_VERSION >>> 8;
    bytes[CLIENT_HELLO_LENGTH - 1] = PROTOCOL_VERSION & 0xff;
    return bytes;
}
function decodeServerHello(bytes) {
    if (bytes.length !== SERVER_HELLO_LENGTH) {
        return Tag("MalformedHello", {
            detail: `gateway hello has ${bytes.length} bytes; expected ${SERVER_HELLO_LENGTH}`,
        });
    }
    for (let index = 0; index < HELLO_MAGIC.length; index += 1) {
        if (bytes[index] !== HELLO_MAGIC[index]) {
            return Tag("MalformedHello", {
                detail: "gateway returned unsupported protocol magic",
            });
        }
    }
    const version = ((bytes[CLIENT_HELLO_LENGTH - 2] ?? 0) << 8) |
        (bytes[CLIENT_HELLO_LENGTH - 1] ?? 0);
    if (version !== PROTOCOL_VERSION) {
        return Tag("MalformedHello", {
            detail: `gateway returned unsupported protocol version ${version}`,
        });
    }
    const idBytes = bytes.slice(CLIENT_HELLO_LENGTH);
    const id = parseRendezvousId(bytesHex(idBytes));
    return id.tag === "Parsed"
        ? Tag("Decoded", { id: id.data, idBytes })
        : Tag("MalformedHello", { detail: "gateway returned a malformed ID" });
}
async function websocketBytes(value, cap) {
    if (value instanceof ArrayBuffer) {
        return value.byteLength <= cap
            ? Tag("Decoded", new Uint8Array(value))
            : Tag("FrameTooLarge", { length: value.byteLength, maximum: cap });
    }
    if (ArrayBuffer.isView(value)) {
        return value.byteLength <= cap
            ? Tag("Decoded", new Uint8Array(value.buffer, value.byteOffset, value.byteLength))
            : Tag("FrameTooLarge", { length: value.byteLength, maximum: cap });
    }
    if (typeof Blob !== "undefined" && value instanceof Blob) {
        if (value.size > cap) {
            return Tag("FrameTooLarge", { length: value.size, maximum: cap });
        }
        try {
            const bytes = new Uint8Array(await value.arrayBuffer());
            return bytes.length <= cap
                ? Tag("Decoded", bytes)
                : Tag("FrameTooLarge", { length: bytes.length, maximum: cap });
        }
        catch (error) {
            return Tag("TransferFailed", {
                direction: "Inbound",
                detail: describeError(error),
            });
        }
    }
    return Tag("UnsupportedFrame", {
        format: typeof value === "string" ? "Text" : "Unknown",
    });
}
function parseRendezvousId(value) {
    return value.length === ID_HEX_LENGTH && /^[0-9a-f]{32}$/.test(value)
        ? Tag("Parsed", value)
        : Tag("Invalid");
}
function exactRecord(value, expectedKeys) {
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
        return false;
    }
    const keys = Object.keys(value).sort();
    const expected = [...expectedKeys].sort();
    return keys.length === expected.length && keys.every((key, index) => key === expected[index]);
}
function browserNetworkFailure(error) {
    if (typeof DOMException !== "undefined" &&
        error instanceof DOMException &&
        (error.name === "NotAllowedError" || error.name === "SecurityError")) {
        return Tag("PermissionDenied", {
            interface: "auto-wifi",
            stage: "TransportOpen",
            detail: describeError(error),
        });
    }
    return Tag("DiscoveryFailed", { detail: describeError(error) });
}
function preferredFailure(failures) {
    return (failures.find((failure) => failure.tag === "PermissionDenied") ??
        failures.find((failure) => failure.tag === "HostApiUnavailable") ??
        failures[0] ??
        Tag("DiscoveryFailed", { detail: "no local Prns browser gateway was discovered" }));
}
function closeProbes(outcomes) {
    for (const outcome of outcomes) {
        if (outcome.tag === "Connected") {
            outcome.data.pending.close();
        }
    }
}
function closeSocket(socket) {
    try {
        socket.close();
    }
    catch {
        return;
    }
}
function isLocalhostUrl(value) {
    try {
        const hostname = new URL(value).hostname.toLowerCase();
        return (hostname === "localhost" ||
            hostname === "[::1]" ||
            hostname.split(".").length === 4 && hostname.split(".")[0] === "127");
    }
    catch {
        return false;
    }
}
function websocketPayloadLength(value) {
    if (value instanceof ArrayBuffer) {
        return Tag("Measured", value.byteLength);
    }
    if (ArrayBuffer.isView(value)) {
        return Tag("Measured", value.byteLength);
    }
    if (typeof Blob !== "undefined" && value instanceof Blob) {
        return Tag("Measured", value.size);
    }
    return Tag("UnsupportedFrame");
}
function describeWebSocketDecodeFailure(failure) {
    return match_into().from(failure, {
        UnsupportedFrame: ({ format }) => `gateway returned an unsupported ${format.toLowerCase()} frame`,
        FrameTooLarge: ({ length, maximum }) => `gateway frame is ${length} bytes; maximum is ${maximum}`,
        TransferFailed: ({ direction, detail }) => `gateway ${direction.toLowerCase()} transfer failed: ${detail}`,
    });
}
function bytesHex(bytes) {
    let value = "";
    for (const byte of bytes) {
        value += byte.toString(16).padStart(2, "0");
    }
    return value;
}
function hexBytes(value) {
    const bytes = new Uint8Array(value.length / 2);
    for (let index = 0; index < bytes.length; index += 1) {
        bytes[index] = Number.parseInt(value.slice(index * 2, index * 2 + 2), 16);
    }
    return bytes;
}
function describeError(error) {
    return error instanceof Error ? error.message : String(error);
}
function wait(ms) {
    return new Promise((resolve) => globalThis.setTimeout(resolve, ms));
}
