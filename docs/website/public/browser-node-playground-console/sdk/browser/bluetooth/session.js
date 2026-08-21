import { Tag, match, match_into } from "../../casework.js";
import { bytesField, record, stringField } from "../decoding.js";
import { describeHostError } from "../host_errors.js";
import { closeFailed, closedSessionOutcome, delay, describeInterfaceSessionFailure, hasCleanupFailures, unexpectedSessionFailure, } from "../session.js";
import { PrnsValidationError, channelTag, packetFrame, } from "../values.js";
import { bluetoothStage, characteristicBytes, disconnectBluetoothServer, writeBluetoothValue, } from "./gatt.js";
const HANDSHAKE_TIMEOUT_MS = 10_000;
const OUTBOUND_POLL_MS = 25;
export class BrowserBluetoothSession {
    name = "bluetooth";
    #host;
    #server;
    #control;
    #data;
    #reassembler;
    #interfaceId;
    #writeQueue = Promise.resolve(Tag("Written"));
    #closed = false;
    #confirmed = false;
    #status = Tag("Negotiating");
    #connectFailure;
    constructor(host, server, control, data) {
        this.#host = host;
        this.#server = server;
        this.#control = control;
        this.#data = data;
        this.#reassembler = host.createBluetoothReassembler();
    }
    get interfaceId() {
        if (!this.#interfaceId) {
            throw new PrnsValidationError("invalid-component", "Bluetooth peer interface is not registered yet");
        }
        return this.#interfaceId;
    }
    get status() {
        return this.#status;
    }
    async start() {
        const controlStarted = await bluetoothStage("Handshake", () => this.#control.startNotifications());
        if (controlStarted.tag !== "Completed") {
            return controlStarted;
        }
        this.#control.addEventListener("characteristicvaluechanged", (event) => {
            try {
                const handled = this.#handleControlEvent(event);
                if (handled.tag !== "Handled") {
                    this.#handleEventFailure(handled);
                }
            }
            catch (error) {
                this.#handleEventFailure(unexpectedSessionFailure(error));
            }
        });
        if (this.#data !== this.#control) {
            const dataStarted = await bluetoothStage("Handshake", () => this.#data.startNotifications());
            if (dataStarted.tag !== "Completed") {
                return dataStarted;
            }
            this.#data.addEventListener("characteristicvaluechanged", (event) => {
                try {
                    const handled = this.#handleDataEvent(event);
                    if (handled.tag !== "Handled") {
                        this.#handleEventFailure(handled);
                    }
                }
                catch (error) {
                    this.#handleEventFailure(unexpectedSessionFailure(error));
                }
            });
        }
        const written = await this.#writeControl(this.#host.bluetoothDialerHello());
        if (written.tag !== "Written") {
            return sessionFailureToConnectFailure("Handshake", written);
        }
        const confirmed = await this.#waitForPeer();
        if (confirmed.tag !== "Confirmed") {
            return confirmed;
        }
        void this.#outboundLoop();
        return Tag("Started");
    }
    async close() {
        if (this.#closed) {
            return closedSessionOutcome(this.#status);
        }
        this.#closed = true;
        const causes = [];
        if (this.#interfaceId) {
            const detached = this.#host.deactivateInterface(this.#interfaceId);
            if (detached.tag !== "Detached") {
                causes.push(Tag("RuntimeDetachFailed", { detail: detached.data.detail }));
            }
        }
        const pendingWrite = await this.#writeQueue;
        if (pendingWrite.tag !== "Written") {
            causes.push(Tag("TransportCloseFailed", {
                detail: describeInterfaceSessionFailure(pendingWrite),
            }));
        }
        const disconnected = disconnectBluetoothServer(this.#server);
        if (disconnected) {
            causes.push(disconnected);
        }
        if (hasCleanupFailures(causes)) {
            const failed = closeFailed(causes);
            this.#status = Tag("Failed", failed);
            return failed;
        }
        this.#status = Tag("Closed");
        return Tag("Closed");
    }
    async #waitForPeer() {
        const started = Date.now();
        while (!this.#confirmed && !this.#closed && !this.#connectFailure) {
            if (Date.now() - started > HANDSHAKE_TIMEOUT_MS) {
                const timedOut = Tag("TimedOut", {
                    interface: "bluetooth",
                    stage: "Handshake",
                    timeoutMs: HANDSHAKE_TIMEOUT_MS,
                });
                this.#abortConnect(timedOut);
                return timedOut;
            }
            await delay(25);
        }
        if (this.#connectFailure) {
            return this.#connectFailure;
        }
        if (!this.#confirmed) {
            return Tag("ConnectionFailed", {
                interface: "bluetooth",
                stage: "Handshake",
                detail: "Bluetooth link closed before peer confirmation",
            });
        }
        return Tag("Confirmed");
    }
    #handleControlEvent(event) {
        const decoded = characteristicBytes(event);
        if (decoded.tag !== "Decoded") {
            return decoded;
        }
        const bytes = decoded.data;
        let control;
        try {
            control = parseBluetoothControl(this.#host.bluetoothDecodeControl(bytes));
        }
        catch (error) {
            return Tag("ProtocolViolation", {
                protocol: "Bluetooth",
                detail: describeHostError(error),
            });
        }
        return match_into().from(control, {
            Hello: () => this.#data === this.#control
                ? this.#handleDataBytes(bytes)
                : Tag("Handled"),
            Welcome: (identity) => {
                if (this.#confirmed) {
                    return Tag("Handled");
                }
                let registration;
                try {
                    registration = {
                        interfaceName: "bluetooth",
                        supervisorKind: "bluetooth-auto",
                        kind: "bluetooth-peer",
                        channelTag: channelTag(identity),
                        bitrateBps: this.#host.bluetoothBitrateBps(),
                        hardwareMtu: this.#host.bluetoothHardwareMtu(),
                    };
                }
                catch (error) {
                    return Tag("ProtocolViolation", {
                        protocol: "Bluetooth",
                        detail: describeHostError(error),
                    });
                }
                const registered = this.#host.registerInterface(registration);
                if (registered.tag !== "Registered") {
                    return registered;
                }
                this.#interfaceId = registered.data;
                this.#confirmed = true;
                this.#status = Tag("Active");
                return Tag("Handled");
            },
            Close: () => {
                void this.close();
                return Tag("Handled");
            },
        });
    }
    #handleDataEvent(event) {
        const decoded = characteristicBytes(event);
        return decoded.tag === "Decoded"
            ? this.#handleDataBytes(decoded.data)
            : decoded;
    }
    #handleDataBytes(bytes) {
        if (!this.#confirmed || !this.#interfaceId) {
            return Tag("Handled");
        }
        let frame;
        try {
            frame = this.#reassembler.absorb(bytes);
        }
        catch (error) {
            return Tag("ProtocolViolation", {
                protocol: "Bluetooth",
                detail: describeHostError(error),
            });
        }
        if (frame && frame.length > 0) {
            const ingested = this.#host.ingest(this.#interfaceId, packetFrame(frame));
            return ingested.tag === "Accepted" ? Tag("Handled") : ingested;
        }
        return Tag("Handled");
    }
    #handleEventFailure(failure) {
        if (!this.#confirmed) {
            this.#abortConnect(failure.tag === "AlreadyActive"
                ? failure
                : sessionFailureToConnectFailure("Handshake", failure));
            return;
        }
        const sessionFailure = failure.tag === "AlreadyActive"
            ? unexpectedSessionFailure(`Bluetooth peer became active more than once for ${failure.data.target}`)
            : failure;
        void this.#fail(sessionFailure);
    }
    #abortConnect(failure) {
        if (this.#closed) {
            return;
        }
        this.#connectFailure = failure;
        this.#status = Tag("Failed", failure.tag === "RuntimeRejected"
            ? failure
            : unexpectedSessionFailure(describeBluetoothConnectFailure(failure)));
        this.#closed = true;
        if (this.#interfaceId) {
            this.#host.deactivateInterface(this.#interfaceId);
        }
        disconnectBluetoothServer(this.#server);
    }
    async #outboundLoop() {
        try {
            while (!this.#closed) {
                const interfaceId = this.#interfaceId;
                if (this.#confirmed && interfaceId) {
                    const outbound = this.#host.takeOutboundFor(interfaceId);
                    if (outbound.tag !== "Outbound") {
                        await this.#fail(outbound);
                        return;
                    }
                    for (const frame of outbound.data) {
                        for (const fragment of this.#host.bluetoothDataFragments(frame.bytes)) {
                            const written = await this.#writeData(fragment);
                            if (written.tag !== "Written") {
                                await this.#fail(written);
                                return;
                            }
                        }
                    }
                }
                await delay(OUTBOUND_POLL_MS);
            }
        }
        catch (error) {
            if (!this.#closed) {
                await this.#fail(unexpectedSessionFailure(error));
            }
        }
    }
    async #fail(sessionFailure) {
        if (this.#closed) {
            return;
        }
        this.#status = Tag("Failed", sessionFailure);
        this.#closed = true;
        if (this.#interfaceId) {
            this.#host.deactivateInterface(this.#interfaceId);
        }
        await this.#writeQueue;
        disconnectBluetoothServer(this.#server);
    }
    async #writeControl(bytes) {
        return this.#write(this.#control, bytes);
    }
    async #writeData(bytes) {
        return this.#write(this.#data, bytes);
    }
    async #write(characteristic, bytes) {
        if (this.#closed || bytes.length === 0) {
            return Tag("Written");
        }
        const write = this.#writeQueue
            .then(async (previous) => {
            if (previous.tag !== "Written" || this.#closed) {
                return previous;
            }
            return writeBluetoothValue(characteristic, bytes);
        })
            .catch((error) => unexpectedSessionFailure(error));
        this.#writeQueue = write;
        return write;
    }
}
function parseBluetoothControl(raw) {
    const object = record(raw, "BluetoothControl");
    const type = stringField(object, "type");
    if (!RAW_CONTROL_TYPES.has(type)) {
        throw new PrnsValidationError("invalid-component", `unknown Bluetooth control ${type}`);
    }
    return match(type, {
        hello: () => Tag("Hello", bytesField(object, "identity")),
        welcome: () => Tag("Welcome", bytesField(object, "identity")),
        close: () => Tag("Close", stringField(object, "reason")),
    });
}
function sessionFailureToConnectFailure(stage, failure) {
    if (failure.tag === "RuntimeRejected") {
        return failure;
    }
    return Tag("ConnectionFailed", {
        interface: "bluetooth",
        stage,
        detail: describeInterfaceSessionFailure(failure),
    });
}
function describeBluetoothConnectFailure(failure) {
    return match(failure, {
        HostApiUnavailable: ({ api }) => `${api} is unavailable`,
        PermissionDenied: ({ detail }) => detail,
        Cancelled: ({ stage }) => `Bluetooth ${stage} was cancelled`,
        UnsupportedDevice: ({ capability }) => `Bluetooth device does not provide ${capability}`,
        TimedOut: ({ stage, timeoutMs }) => `Bluetooth ${stage} timed out after ${timeoutMs}ms`,
        ConnectionFailed: ({ detail }) => detail,
        AlreadyActive: ({ target }) => `${target} is already active`,
        StableIdentityUnavailable: ({ detail }) => detail,
        RuntimeRejected: ({ operation, detail }) => `${operation}: ${detail}`,
    });
}
const RAW_CONTROL_TYPES = new Set(["hello", "welcome", "close"]);
