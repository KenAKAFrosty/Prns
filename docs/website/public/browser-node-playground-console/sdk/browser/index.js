import { Tag, from, match, match_into } from "../casework.js";
import { BoundedAsyncLane } from "../async_lanes.js";
import { DESTINATION_HASH_LENGTH, HOST_CONTRACT_ABI, HOST_SCHEMA_VERSION, INTERFACE_ID_LENGTH, PRODUCT_VERSION, RESOURCE_HASH_LENGTH, SAFE_INT_MAX, SAFE_INT_MIN, balancedLimits, destinationHash, identityHash, interfaceId, linkId, packetHash, requestId, requestPathHash, resourceHash, } from "../contract.js";
import { browserLimits, bundledWasmModuleUrl, cooperativeBackendInfo, loadBundledWasm, loadOrCreateBleIdentity, webCryptoEntropy, webCryptoIdentity, } from "./bootstrap.js";
import { byteKey } from "./bytes.js";
import { commandFailed, } from "./command_settlement.js";
import { parseEvent } from "./events.js";
import { PrnsInterfaces } from "./interfaces.js";
import { RuntimeHost, fillEntropy, runtimeRejected, saturatingAdd, } from "./runtime.js";
import { describeHostError } from "./host_errors.js";
import { BROWSER_PERSISTENCE_VERSION, BrowserLocalStorageBleIdentityStore, browserPersistenceStores, describePersistenceStoreFailure, parseBrowserPersistedState, parsePersistenceRestoreReport, } from "./persistence.js";
import { blobResourceSource, byteResourceSource, sendResourceFromSource, } from "./resource_send.js";
import { browserResourceCompressor } from "./resource_compressor.js";
import { describeInterfaceSessionFailure } from "./session.js";
import { parseSnapshot } from "./snapshot.js";
import { BROWSER_RENDEZVOUS_FRAMING_SELECTION, } from "./websocket/index.js";
import { BLE_IDENTITY_LENGTH, MIN_ENTROPY_BYTES, PrnsValidationError, appData, appName, aspect, bitrateBps, bleIdentity, channelTag, commandId, entropyBytes, hardwareMtu, hopCount, identitySecretKey, nonNegativeInteger, nowMillis, packetFrame, positiveInteger, } from "./values.js";
export { Tag, from, match, match_into };
export { DESTINATION_HASH_LENGTH, HOST_CONTRACT_ABI, HOST_SCHEMA_VERSION, INTERFACE_ID_LENGTH, PRODUCT_VERSION, RESOURCE_HASH_LENGTH, SAFE_INT_MAX, SAFE_INT_MIN, balancedLimits, destinationHash, identityHash, interfaceId, linkId, packetHash, requestId, requestPathHash, resourceHash, };
export { AutoWifiController, AutoWifiInterface, parseBrowserGatewayCatalog, validateBrowserGatewayUrl, } from "./auto_wifi/index.js";
export { webCryptoEntropy } from "./bootstrap.js";
export { PrnsInterfaces } from "./interfaces.js";
export { BluetoothInterface } from "./bluetooth/index.js";
export { UsbAutoInterface } from "./usb_auto/index.js";
export { BROWSER_PERSISTENCE_VERSION, BrowserLocalStorageBleIdentityStore, BrowserLocalStorageIdentityStore, BrowserLocalStoragePersistenceStore, } from "./persistence.js";
export { RNodeInterface } from "./rnode.js";
export { WebSocketInterface } from "./websocket/index.js";
export { BLE_IDENTITY_LENGTH, MIN_ENTROPY_BYTES, PrnsValidationError, appData, appName, aspect, bitrateBps, bleIdentity, channelTag, commandId, entropyBytes, hardwareMtu, hopCount, identitySecretKey, nowMillis, packetFrame, } from "./values.js";
export function persistentBrowser(root = "prns") {
    return browserPersistenceStores(root);
}
export class Prns {
    interfaces;
    #runtime;
    #host;
    #entropy;
    #now;
    #startedAtMillis;
    #limits;
    #resourceCompressionModuleUrl;
    #events;
    #diagnostics;
    #pendingCommands = new Map();
    #responseParts = new Map();
    #attachedInterfaces = new Map();
    #lifecycle = Tag("Running");
    #stopCompleted = false;
    #stopPromise;
    #persistenceStore;
    #persistenceRestored;
    #lastPersistenceFlushCause;
    #persistenceFailureDetail;
    constructor(wasm, runtime, entropy, now, bleIdentityAvailability, limits, resourceCompressionModuleUrl, persistenceStore, persistenceRestored, restorationReport) {
        this.#runtime = runtime;
        this.#entropy = entropy;
        this.#now = now;
        this.#startedAtMillis = now();
        this.#limits = limits;
        this.#resourceCompressionModuleUrl =
            resourceCompressionModuleUrl.href;
        this.#persistenceStore = persistenceStore;
        this.#persistenceRestored = persistenceRestored;
        this.#events = new BoundedAsyncLane({
            name: "ApplicationEvents",
            maximumValues: limits.applicationEvents,
            maximumBytes: limits.retainedEventBytes,
            measure: retainedBrowserEventBytes,
            onRejected: (rejectedEventBytes) => this.#failBackpressure(rejectedEventBytes),
            onBeforeNext: () => this.#pumpEvents(),
        });
        this.#diagnostics = new BoundedAsyncLane({
            name: "Diagnostics",
            maximumValues: limits.diagnostics,
            maximumBytes: Number.MAX_SAFE_INTEGER,
            measure: () => 0,
            gap: (count) => Tag("DiagnosticsDropped", { count }),
            onBeforeNext: () => this.#pumpEvents(),
        });
        this.#host = new RuntimeHost(wasm, runtime, entropy, now, bleIdentityAvailability, () => this.#pumpEvents());
        this.interfaces = new PrnsInterfaces(this.#host);
        if (restorationReport !== undefined) {
            this.#diagnostics.push(Tag("PersistenceRestored", restorationReport));
        }
    }
    static async create(options) {
        const loaded = options.wasm
            ? Tag("Loaded", options.wasm)
            : await loadBundledWasm();
        if (loaded.tag !== "Loaded") {
            return loaded;
        }
        const wasm = loaded.data;
        let actualAbi;
        let actualSchemaVersion;
        let actualPersistenceVersion;
        let actualProductVersion;
        try {
            actualAbi = wasm.hostContractAbi();
            actualSchemaVersion = wasm.hostSchemaVersion();
            actualPersistenceVersion = wasm.browserPersistenceVersion();
            actualProductVersion = wasm.productVersion();
        }
        catch (error) {
            return runtimeRejected("initialize", error);
        }
        if (actualAbi !== HOST_CONTRACT_ABI ||
            actualSchemaVersion !== HOST_SCHEMA_VERSION ||
            actualProductVersion !== PRODUCT_VERSION) {
            return Tag("ContractMismatch", {
                requiredAbi: HOST_CONTRACT_ABI,
                actualAbi,
                requiredSchemaVersion: HOST_SCHEMA_VERSION,
                actualSchemaVersion,
                requiredProductVersion: PRODUCT_VERSION,
                actualProductVersion,
            });
        }
        if (actualPersistenceVersion !== BROWSER_PERSISTENCE_VERSION) {
            return runtimeRejected("initialize", `browser persistence version ${actualPersistenceVersion} does not match ${BROWSER_PERSISTENCE_VERSION}`);
        }
        let identityLength;
        try {
            identityLength = positiveInteger(wasm.identitySecretKeyLength(), "identity secret key length");
        }
        catch (error) {
            return runtimeRejected("initialize", error);
        }
        const store = options.identityStore;
        let identity;
        if (store) {
            let loaded;
            try {
                loaded = await store.load(identityLength);
            }
            catch (error) {
                return Tag("IdentityStoreFailed", {
                    operation: "Load",
                    detail: describeHostError(error),
                });
            }
            if (loaded.tag === "Loaded") {
                try {
                    identity = identitySecretKey(loaded.data, identityLength);
                }
                catch (error) {
                    return Tag("StoredIdentityInvalid", {
                        detail: describeHostError(error),
                    });
                }
            }
            else if (loaded.tag !== "Missing") {
                return loaded;
            }
        }
        if (!identity) {
            const generated = webCryptoIdentity(identityLength);
            if (generated.tag !== "Generated") {
                return generated;
            }
            identity = generated.data;
            if (store) {
                let saved;
                try {
                    saved = await store.save(identity);
                }
                catch (error) {
                    return Tag("IdentityStoreFailed", {
                        operation: "Save",
                        detail: describeHostError(error),
                    });
                }
                if (saved.tag !== "Saved") {
                    return saved;
                }
            }
        }
        const bleIdentityAvailability = await loadOrCreateBleIdentity(options.bleIdentityStore ?? new BrowserLocalStorageBleIdentityStore());
        const bleIdentity = bleIdentityAvailability.tag === "Available"
            ? bleIdentityAvailability.data
            : undefined;
        const persistenceStore = options.persistenceStore;
        let persistedState;
        if (persistenceStore !== undefined) {
            let loaded;
            try {
                loaded = await persistenceStore.load();
            }
            catch (error) {
                return Tag("PersistenceStoreFailed", {
                    operation: "Load",
                    detail: describeHostError(error),
                });
            }
            if (loaded.tag === "Loaded") {
                try {
                    persistedState = parseBrowserPersistedState(loaded.data);
                }
                catch (error) {
                    return Tag("StoredPersistenceInvalid", {
                        detail: describeHostError(error),
                    });
                }
            }
            else if (loaded.tag !== "Missing") {
                return loaded;
            }
        }
        let limits;
        let now;
        let runtime;
        try {
            limits = browserLimits(options.limits ?? balancedLimits());
            now = options.now ?? nowMillis;
            runtime = new wasm.PrnsRuntime(identity, bleIdentity);
        }
        catch (error) {
            return runtimeRejected("initialize", error);
        }
        let restorationReport;
        if (persistedState !== undefined) {
            try {
                restorationReport = parsePersistenceRestoreReport(runtime.restorePersistedState({
                    ...persistedState,
                    nowMs: nowMillis(Math.max(now(), persistedState.takenAtMillis)),
                }));
            }
            catch (error) {
                return Tag("StoredPersistenceInvalid", {
                    detail: describeHostError(error),
                });
            }
        }
        try {
            return Tag("Ready", new Prns(wasm, runtime, options.entropy ?? webCryptoEntropy, now, bleIdentityAvailability, limits, options.resourceCompressionModuleUrl ??
                bundledWasmModuleUrl(), persistenceStore, persistedState !== undefined, restorationReport));
        }
        catch (error) {
            return runtimeRejected("initialize", error);
        }
    }
    registerSingleDestination(options) {
        try {
            return Tag("Registered", destinationHash(this.#runtime.registerSingleDestination(options)));
        }
        catch (error) {
            return runtimeRejected("register-destination", error);
        }
    }
    registerNodePage(appData) {
        try {
            return Tag("Registered", destinationHash(this.#runtime.registerNodePage({ appData })));
        }
        catch (error) {
            return runtimeRejected("register-node-page", error);
        }
    }
    execute(command) {
        return this.#execute(command);
    }
    #execute(command) {
        if (this.#lifecycle.tag !== "Running") {
            return Promise.resolve(commandFailed(Tag("NodeStopped")));
        }
        return match_into().from(command, {
            Announce: ({ destination, interface: interfaceId }) => this.#issueCommand("announce", command, (entropy) => this.#runtime.announce({
                destination,
                ...(interfaceId === undefined ? {} : { interfaceId }),
                nowMs: this.#now(),
                entropy,
            })),
            SendSinglePacket: ({ destination, payload }) => this.#issueCommand("send-single-packet", command, (entropy) => this.#runtime.sendSinglePacket({
                destination,
                payload,
                nowMs: this.#now(),
                entropy,
            })),
            CloseLink: ({ linkId: value }) => this.#issueCommand("close-link", command, (entropy) => this.#runtime.closeLink({
                linkId: value,
                nowMs: this.#now(),
                entropy,
            })),
            AttachTcpServer: async () => commandFailed(Tag("UnsupportedByBackend")),
            AttachTcpClient: async () => commandFailed(Tag("UnsupportedByBackend")),
            AttachUdp: async () => commandFailed(Tag("UnsupportedByBackend")),
            AttachInterface: ({ config, routing }) => this.#attachInterface(config, routing),
            DetachInterface: ({ interface: interfaceId }) => this.#detachInterface(interfaceId),
            EstablishLink: ({ destination }) => this.#issueCommand("establish-link", command, (entropy) => this.#runtime.establishLink({
                destination,
                nowMs: this.#now(),
                entropy,
            })),
            RequestPath: ({ destination }) => this.#issueCommand("request-path", command, (entropy) => this.#runtime.requestPath({
                destination,
                nowMs: this.#now(),
                entropy,
            })),
            Identify: ({ linkId: value, identity }) => this.#issueCommand("identify", command, (entropy) => this.#runtime.identify({
                linkId: value,
                identity,
                nowMs: this.#now(),
                entropy,
            })),
            SendLinkPacket: ({ linkId: value, payload }) => this.#issueCommand("send-link-packet", command, (entropy) => this.#runtime.sendLinkPacket({
                linkId: value,
                payload,
                nowMs: this.#now(),
                entropy,
            })),
            Request: ({ linkId: value, pathHash, payload, timeout, maximumResponseBytes, }) => this.#issueCommand("request", command, (entropy) => this.#runtime.request({
                linkId: value,
                pathHash,
                payload,
                nowMs: this.#now(),
                entropy,
                ...runtimeResponseTimeout(timeout),
                ...(maximumResponseBytes === undefined
                    ? {}
                    : {
                        maximumResponseBytes: nonNegativeInteger(maximumResponseBytes, "maximumResponseBytes"),
                    }),
            })),
            Respond: ({ linkId: value, requestId: responseRequestId, requestRttMillis, payload, }) => this.#issueCommand("respond", command, (entropy) => this.#runtime.respond({
                linkId: value,
                requestId: responseRequestId,
                requestRttMillis,
                payload,
                nowMs: this.#now(),
                entropy,
            })),
            SendResource: ({ linkId: value, payload, packedMetadata, compression, }) => this.#sendResourceSource(value, byteResourceSource(payload), compression, packedMetadata),
            SetLinkResourceStrategy: ({ linkId: value, strategy }) => this.#issueCommand("set-link-resource-strategy", command, (entropy) => this.#runtime.setLinkResourceStrategy({
                linkId: value,
                nowMs: this.#now(),
                entropy,
                ...runtimeResourceStrategy(strategy),
            })),
            SetDestinationResourceStrategy: async ({ destination, strategy, }) => {
                try {
                    const configured = this.#runtime.setDestinationResourceStrategy({
                        destination,
                        ...runtimeResourceStrategy(strategy),
                    });
                    return configured
                        ? Tag("Succeeded", Tag("ResourceStrategySet"))
                        : commandFailed(Tag("UnknownDestination"));
                }
                catch (error) {
                    return commandFailed(browserCommandFailure("set-destination-resource-strategy", error));
                }
            },
            SendChannelMessage: ({ linkId: value, messageType, payload, }) => {
                if (!Number.isSafeInteger(messageType) ||
                    messageType < 0 ||
                    messageType > 0xefff) {
                    return Promise.resolve(commandFailed(Tag("InvalidChannelMessageType")));
                }
                return this.#issueCommand("send-channel-message", command, (entropy) => this.#runtime.sendChannelMessage({
                    linkId: value,
                    messageType,
                    payload,
                    nowMs: this.#now(),
                    entropy,
                }));
            },
            AllowRequester: ({ destination, pathHash, identity }) => this.#issueCommand("allow-requester", command, (entropy) => this.#runtime.allowRequester({
                destination,
                pathHash,
                identity,
                nowMs: this.#now(),
                entropy,
            })),
        });
    }
    announce(destination, interfaceId) {
        return this.execute(Tag("Announce", interfaceId === undefined
            ? { destination }
            : { destination, interface: interfaceId }));
    }
    sendSinglePacket(destination, payload) {
        return this.execute(Tag("SendSinglePacket", { destination, payload }));
    }
    closeLink(value) {
        return this.execute(Tag("CloseLink", { linkId: value }));
    }
    attachInterface(config, routing) {
        return this.execute(routing === undefined
            ? Tag("AttachInterface", { config })
            : Tag("AttachInterface", { config, routing }));
    }
    detachInterface(interfaceId) {
        return this.execute(Tag("DetachInterface", { interface: interfaceId }));
    }
    establishLink(destination) {
        return this.execute(Tag("EstablishLink", { destination }));
    }
    requestPath(destination) {
        return this.execute(Tag("RequestPath", { destination }));
    }
    identify(value, identity) {
        return this.execute(Tag("Identify", { linkId: value, identity }));
    }
    sendLinkPacket(value, payload) {
        return this.execute(Tag("SendLinkPacket", { linkId: value, payload }));
    }
    request(value, pathHash, payload, timeout = Tag("LinkDefault"), maximumResponseBytes) {
        return this.execute(Tag("Request", {
            linkId: value,
            pathHash,
            payload,
            timeout,
            ...(maximumResponseBytes === undefined
                ? {}
                : { maximumResponseBytes }),
        }));
    }
    respond(value, responseRequestId, requestRttMillis, payload) {
        return this.execute(Tag("Respond", {
            linkId: value,
            requestId: responseRequestId,
            requestRttMillis,
            payload,
        }));
    }
    sendResource(value, payload, options = {}) {
        return this.execute(Tag("SendResource", {
            linkId: value,
            payload,
            compression: options.compression ?? Tag("Auto"),
            ...(options.packedMetadata === undefined
                ? {}
                : { packedMetadata: options.packedMetadata }),
        }));
    }
    sendResourceBlob(value, blob, options = {}) {
        return this.#sendResourceSource(value, blobResourceSource(blob), options.compression ?? Tag("Auto"), options.packedMetadata);
    }
    setLinkResourceStrategy(value, strategy) {
        return this.execute(Tag("SetLinkResourceStrategy", { linkId: value, strategy }));
    }
    setDestinationResourceStrategy(destination, strategy) {
        return this.execute(Tag("SetDestinationResourceStrategy", {
            destination,
            strategy,
        }));
    }
    sendChannelMessage(value, messageType, payload) {
        return this.execute(Tag("SendChannelMessage", {
            linkId: value,
            messageType,
            payload,
        }));
    }
    allowRequester(destination, pathHash, identity) {
        return this.execute(Tag("AllowRequester", { destination, pathHash, identity }));
    }
    get lifecycle() {
        return this.#lifecycle;
    }
    get backendInfo() {
        return cooperativeBackendInfo();
    }
    get capabilities() {
        const info = this.backendInfo;
        return Tag("Cooperative", {
            available: new Set(info.capabilities),
            interfaceKinds: new Set(info.interfaceKinds),
        });
    }
    stop() {
        if (this.#stopCompleted) {
            return Promise.resolve(Tag("AlreadyStopped"));
        }
        if (this.#stopPromise !== undefined) {
            return this.#stopPromise;
        }
        this.#stopPromise = this.#performStop();
        return this.#stopPromise;
    }
    claimEvents() {
        this.#pumpEvents();
        return this.#events.claim();
    }
    claimDiagnostics() {
        this.#pumpEvents();
        return this.#diagnostics.claim();
    }
    snapshot() {
        try {
            return Tag("Captured", parseSnapshot(this.#runtime.snapshot()));
        }
        catch (error) {
            return runtimeRejected("snapshot", error);
        }
    }
    hostSnapshot() {
        try {
            const snapshot = parseSnapshot(this.#runtime.snapshot());
            const inspection = this.#host.interfaceInspection();
            const running = this.#lifecycle.tag === "Running";
            const health = running ? "Connected" : "Disabled";
            const interfaces = snapshot.interfaces.map((entry) => {
                const active = inspection.get(byteKey(entry.id));
                return {
                    interfaceId: entry.id,
                    ...(active === undefined ? {} : { name: active.name }),
                    ...(active?.kind === undefined ? {} : { kind: active.kind }),
                    health,
                    rxBytes: BigInt(active?.rxBytes ?? 0),
                    txBytes: BigInt(active?.txBytes ?? 0),
                    routeCount: entry.routes,
                    linkCount: entry.links,
                    transportedLinkCount: entry.transportedLinks,
                };
            });
            const interfaceCount = interfaces.length;
            const onlineInterfaceCount = running ? interfaceCount : 0;
            const transportedLinkCount = interfaces.reduce((total, entry) => saturatingAdd(total, entry.transportedLinkCount), 0);
            const rxBytes = interfaces.reduce((total, entry) => total + entry.rxBytes, 0n);
            const txBytes = interfaces.reduce((total, entry) => total + entry.txBytes, 0n);
            return Tag("Captured", {
                revision: snapshot.revision,
                backend: this.backendInfo,
                interfaces,
                routes: snapshot.routeSnapshots,
                activeLinkCount: snapshot.activeLinkCount,
                destinationIdentities: snapshot.destinationIdentities,
                runtime: {
                    running,
                    uptimeMillis: Math.max(0, this.#now() - this.#startedAtMillis),
                    interfaceCount,
                    onlineInterfaceCount,
                    routeCount: snapshot.routeSnapshots.length,
                    linkCount: snapshot.activeLinkCount,
                    transportedLinkCount,
                    rxBytes,
                    txBytes,
                    rxBps: 0,
                    txBps: 0,
                },
                persistence: {
                    persistent: this.#persistenceStore !== undefined,
                    restored: this.#persistenceRestored,
                    ...(this.#lastPersistenceFlushCause === undefined
                        ? {}
                        : { lastFlushCause: this.#lastPersistenceFlushCause }),
                    ...(this.#persistenceFailureDetail === undefined
                        ? {}
                        : { lastFailureDetail: this.#persistenceFailureDetail }),
                },
            });
        }
        catch (error) {
            return runtimeRejected("snapshot", error);
        }
    }
    async #performStop() {
        const preserveFailure = this.#lifecycle.tag === "Failed";
        if (!preserveFailure) {
            this.#lifecycle = Tag("Stopping");
        }
        for (const pending of this.#pendingCommands.values()) {
            pending.settle(commandFailed(Tag("NodeStopped")));
        }
        this.#pendingCommands.clear();
        this.#responseParts.clear();
        const sessions = [...this.#attachedInterfaces.values()];
        this.#attachedInterfaces.clear();
        const failures = (await Promise.all(sessions.map(async (session) => {
            try {
                const closed = await session.close();
                return closed.tag === "Closed"
                    ? undefined
                    : describeInterfaceSessionFailure(closed);
            }
            catch (error) {
                return describeHostError(error);
            }
        }))).filter((failure) => failure !== undefined);
        if (this.#persistenceStore !== undefined) {
            let failure;
            try {
                const state = parseBrowserPersistedState(this.#runtime.persistedState({ nowMs: this.#now() }));
                const saved = await this.#persistenceStore.save(state);
                if (saved.tag !== "Saved") {
                    failure = describePersistenceStoreFailure(saved);
                }
            }
            catch (error) {
                failure = describeHostError(error);
            }
            if (failure === undefined) {
                this.#lastPersistenceFlushCause = "Shutdown";
                this.#persistenceFailureDetail = undefined;
                this.#diagnostics.push(Tag("PersistenceFlushed", {
                    cause: "Shutdown",
                    target: "RoutingState",
                }));
                this.#diagnostics.push(Tag("PersistenceFlushed", {
                    cause: "Shutdown",
                    target: "Ratchets",
                }));
            }
            else {
                this.#persistenceFailureDetail = failure;
                this.#diagnostics.push(Tag("PersistenceFlushFailed", {
                    cause: "Shutdown",
                    target: "RoutingState",
                }));
                this.#diagnostics.push(Tag("PersistenceFlushFailed", {
                    cause: "Shutdown",
                    target: "Ratchets",
                }));
                failures.push(`flush persistence: ${failure}`);
            }
        }
        this.#events.finish();
        this.#diagnostics.finish();
        this.#stopCompleted = true;
        if (failures.length > 0) {
            const detail = failures.join("; ");
            this.#lifecycle = Tag("Failed", { cause: "BackendFailed", detail });
            return Tag("OperationFailed", { operation: "stop", detail });
        }
        if (!preserveFailure) {
            this.#lifecycle = Tag("Stopped", { reason: "Requested" });
        }
        return Tag("Stopped");
    }
    #attachInterface(config, routing) {
        const unsupported = async () => commandFailed(Tag("UnsupportedByBackend"));
        return match_into().from(config, {
            AutoLan: unsupported,
            TcpClient: unsupported,
            TcpServer: unsupported,
            Udp: unsupported,
            Serial: unsupported,
            Kiss: unsupported,
            Ax25Kiss: unsupported,
            RNode: unsupported,
            MultiRNode: unsupported,
            Pipe: unsupported,
            BackboneClient: unsupported,
            BackboneServer: unsupported,
            I2p: unsupported,
            Weave: unsupported,
            AutomaticUsb: unsupported,
            AutomaticBluetoothLe: unsupported,
            WebSocketClient: ({ target, framing }) => this.#attachWebSocket(target, "WebSocketClient", framing, routing),
            WebSocketServer: unsupported,
            BrowserRendezvous: ({ url }) => this.#attachWebSocket(url, "BrowserRendezvous", BROWSER_RENDEZVOUS_FRAMING_SELECTION, routing),
        });
    }
    async #attachWebSocket(target, kind, framing, routing) {
        const connected = await this.interfaces.webSocket.connect(target, routing === undefined ? { framing } : { framing, routing });
        if (connected.tag !== "Connected") {
            return commandFailed(webSocketCommandFailure(connected));
        }
        const session = connected.data;
        const key = byteKey(session.interfaceId);
        if (this.#attachedInterfaces.has(key)) {
            await session.close();
            return commandFailed(Tag("BackendFailed", {
                detail: `runtime reused active interface identifier ${key}`,
            }));
        }
        this.#host.setContractKind(session.interfaceId, kind);
        this.#attachedInterfaces.set(key, session);
        return Tag("Succeeded", Tag("InterfaceAttached", { interface: session.interfaceId }));
    }
    async #detachInterface(interfaceId) {
        const key = byteKey(interfaceId);
        const session = this.#attachedInterfaces.get(key);
        if (session === undefined) {
            return commandFailed(Tag("UnknownInterface"));
        }
        this.#attachedInterfaces.delete(key);
        const closed = await session.close();
        if (closed.tag !== "Closed") {
            return commandFailed(Tag("BackendFailed", {
                detail: describeInterfaceSessionFailure(closed),
            }));
        }
        return Tag("Succeeded", Tag("InterfaceDetached", { interface: interfaceId }));
    }
    #entropyBytes() {
        return fillEntropy(this.#entropy, MIN_ENTROPY_BYTES);
    }
    #issueCommand(operation, command, issue) {
        return this.#issuePendingCommand(operation, Tag("HostCommand", { command }), issue);
    }
    #issueResourceSegment(input) {
        return this.#issuePendingCommand("send-resource", Tag("ResourceSegment"), (entropy) => this.#runtime.sendResourceSegment({
            ...input,
            nowMs: this.#now(),
            entropy,
        }));
    }
    #issuePendingCommand(operation, pending, issue) {
        if (this.#lifecycle.tag !== "Running") {
            return Promise.resolve(commandFailed(Tag("NodeStopped")));
        }
        if (this.#pendingCommands.size >= this.#limits.pendingCommands) {
            return Promise.resolve(commandFailed(Tag("Busy")));
        }
        const entropy = this.#entropyBytes();
        if (entropy.tag !== "Filled") {
            return Promise.resolve(commandFailed(Tag("EntropyUnavailable")));
        }
        let id;
        try {
            id = commandId(issue(entropy.data));
        }
        catch (error) {
            return Promise.resolve(commandFailed(browserCommandFailure(operation, error)));
        }
        return new Promise((settle) => {
            this.#pendingCommands.set(id, { pending, settle });
            this.#host.notifyRuntimeActivity();
        });
    }
    #sendResourceSource(value, source, compression, packedMetadata) {
        if (this.#lifecycle.tag !== "Running") {
            return Promise.resolve(Tag("Failed", Tag("NodeStopped")));
        }
        return sendResourceFromSource(value, source, compression, packedMetadata, {
            maximumInFlightSegments: this.#limits.pendingCommands,
            plan: (input) => this.#runtime.resourceSegmentPlan(input),
            compress: (payload, metadata) => browserResourceCompressor.compress(payload, metadata, this.#resourceCompressionModuleUrl),
            issue: (input) => this.#issueResourceSegment(input),
        });
    }
    #pumpEvents() {
        if (this.#lifecycle.tag === "Failed" || this.#lifecycle.tag === "Stopped") {
            return;
        }
        let parsed;
        try {
            parsed = this.#runtime.drainEvents().map(parseEvent);
        }
        catch (error) {
            this.#failContract(describeHostError(error));
            return;
        }
        for (const event of parsed) {
            match(event, {
                Application: (application) => {
                    this.#events.push(application);
                },
                Diagnostic: (diagnostic) => {
                    this.#diagnostics.push(diagnostic);
                },
                CommandResponse: ({ commandId: responseCommandId, event }) => {
                    this.#events.push(event);
                    this.#responseParts.set(responseCommandId, [event.data.data]);
                },
                CommandResponseSegment: ({ commandId: responseCommandId, event, }) => {
                    this.#events.push(event);
                    const parts = this.#responseParts.get(responseCommandId) ?? [];
                    parts.push(event.data.data);
                    this.#responseParts.set(responseCommandId, parts);
                },
                CommandSettled: ({ commandId, settlement }) => {
                    if (settlement === undefined) {
                        return;
                    }
                    const pending = this.#pendingCommands.get(commandId);
                    if (pending === undefined) {
                        return;
                    }
                    this.#pendingCommands.delete(commandId);
                    pending.settle(match(pending.pending, {
                        HostCommand: ({ command }) => this.#commandSettlement(commandId, command, settlement),
                        ResourceSegment: () => settlement,
                    }));
                },
            });
        }
    }
    #commandSettlement(id, command, settlement) {
        if (settlement.tag === "Failed") {
            this.#responseParts.delete(id);
            return settlement;
        }
        if (command.tag === "Request") {
            if (settlement.data.tag !== "PacketDelivered") {
                this.#responseParts.delete(id);
                return commandFailed(Tag("WriteFailed", {
                    detail: "request settled without delivery evidence",
                }));
            }
            const parts = this.#responseParts.get(id);
            this.#responseParts.delete(id);
            if (parts === undefined) {
                return commandFailed(Tag("WriteFailed", {
                    detail: "request settled without response data",
                }));
            }
            return Tag("Succeeded", Tag("ResponseReceived", {
                data: concatenateBytes(parts),
                rttMillis: settlement.data.data.rttMillis,
            }));
        }
        if (command.tag === "Respond") {
            if (settlement.data.tag !== "ResponseSent") {
                return commandFailed(Tag("WriteFailed", {
                    detail: "response settled with an unexpected outcome",
                }));
            }
            return Tag("Succeeded", Tag("ResponseSent", {
                rttMillis: command.data.requestRttMillis,
            }));
        }
        return settlement;
    }
    #failBackpressure(rejectedEventBytes) {
        this.#lifecycle = Tag("Failed", {
            cause: "EventBackpressureExceeded",
            limits: this.#limits,
            rejectedEventBytes,
        });
        this.#events.finish();
        this.#diagnostics.finish();
        this.#settleFailedCommands("application event backpressure exceeded");
    }
    #failContract(detail) {
        this.#lifecycle = Tag("Failed", {
            cause: "ContractViolated",
            detail,
        });
        const error = new Error(detail);
        this.#events.fail(error);
        this.#diagnostics.fail(error);
        this.#settleFailedCommands(detail);
    }
    #settleFailedCommands(detail) {
        for (const pending of this.#pendingCommands.values()) {
            pending.settle(commandFailed(Tag("WriteFailed", { detail })));
        }
        this.#pendingCommands.clear();
        this.#responseParts.clear();
    }
}
function browserCommandFailure(operation, error) {
    const detail = describeHostError(error);
    if (detail.includes("payload exceeds")) {
        return Tag("PayloadTooLarge");
    }
    return Tag("WriteFailed", { detail: `${operation}: ${detail}` });
}
function runtimeResponseTimeout(timeout) {
    return match(timeout, {
        LinkDefault: () => ({}),
        Exact: ({ millis }) => ({
            timeoutMillis: nonNegativeInteger(millis, "timeoutMillis"),
        }),
    });
}
function runtimeResourceStrategy(strategy) {
    return match(strategy, {
        Refuse: () => ({ strategy: "refuse" }),
        Accept: ({ maximumUncompressedBytes, acceptCompressed, }) => ({
            strategy: "accept",
            maximumUncompressedBytes: nonNegativeInteger(maximumUncompressedBytes, "maximumUncompressedBytes"),
            acceptCompressed,
        }),
    });
}
function concatenateBytes(parts) {
    const length = parts.reduce((total, part) => total + part.length, 0);
    const joined = new Uint8Array(length);
    let offset = 0;
    for (const part of parts) {
        joined.set(part, offset);
        offset += part.length;
    }
    return joined;
}
function webSocketCommandFailure(failure) {
    return match_into().from(failure, {
        HostApiUnavailable: ({ api }) => Tag("DeviceUnavailable", { detail: `${api} is unavailable` }),
        PermissionDenied: ({ detail }) => Tag("PermissionDenied", { detail }),
        Cancelled: ({ stage }) => Tag("ConnectFailed", { detail: `WebSocket ${stage} was cancelled` }),
        AlreadyActive: ({ target }) => Tag("BackendFailed", { detail: `${target} is already active` }),
        InvalidTarget: ({ detail }) => Tag("InvalidConfiguration", { detail }),
        TimedOut: ({ stage, timeoutMs }) => Tag("ConnectFailed", {
            detail: `WebSocket ${stage} timed out after ${timeoutMs}ms`,
        }),
        ConnectionFailed: ({ detail }) => Tag("ConnectFailed", { detail }),
        RuntimeRejected: ({ operation, detail }) => Tag("BackendFailed", { detail: `${operation}: ${detail}` }),
    });
}
function retainedBrowserEventBytes(event) {
    return match_into().from(event, {
        SingleDelivery: ({ plaintext }) => plaintext.length,
        LinkDelivery: ({ plaintext }) => plaintext.length,
        Request: ({ data }) => data.length,
        Response: ({ data }) => data.length,
        ResponseSegment: ({ data }) => data.length,
        ResourceAvailable: ({ resource, metadata }) => exactBytesAsSafeNumber(resource.totalBytes, "resource.totalBytes") +
            (metadata?.length ?? 0),
        ResourceSegment: ({ data, metadata }) => data.length + (metadata?.length ?? 0),
        ResourceNeedsDecompression: ({ stream }) => stream.length,
        ChannelMessage: ({ data }) => data.length,
    });
}
function exactBytesAsSafeNumber(value, name) {
    if (value > BigInt(Number.MAX_SAFE_INTEGER)) {
        throw new PrnsValidationError("invalid-number", `${name} exceeds the JavaScript safe-integer limit`);
    }
    return Number(value);
}
