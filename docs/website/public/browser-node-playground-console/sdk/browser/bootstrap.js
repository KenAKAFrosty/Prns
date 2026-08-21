import { Tag } from "../casework.js";
import { record } from "./decoding.js";
import { describeHostError } from "./host_errors.js";
import { hostGlobal } from "./host_apis.js";
import { describeStableIdentityStoreFailure } from "./persistence.js";
import { BLE_IDENTITY_LENGTH, MIN_ENTROPY_BYTES, PrnsValidationError, bleIdentity, identitySecretKey, positiveInteger, } from "./values.js";
export function webCryptoEntropy(length) {
    try {
        if (!hostGlobal().crypto) {
            return Tag("HostApiUnavailable", { api: "Crypto" });
        }
        const bytes = webCryptoBytes(length);
        if (bytes.length < MIN_ENTROPY_BYTES) {
            return Tag("InsufficientEntropy", {
                minimum: MIN_ENTROPY_BYTES,
                actual: bytes.length,
            });
        }
        return Tag("Filled", bytes);
    }
    catch (error) {
        return Tag("EntropySourceFailed", { detail: describeHostError(error) });
    }
}
function webCryptoBytes(length) {
    if (!Number.isSafeInteger(length) || length <= 0) {
        throw new PrnsValidationError("invalid-number", "random byte length must be a positive safe integer");
    }
    const out = new Uint8Array(length);
    const crypto = hostGlobal().crypto;
    if (!crypto) {
        throw new PrnsValidationError("missing-host-api", "Prns entropy requires globalThis.crypto.getRandomValues");
    }
    crypto.getRandomValues(out);
    return out;
}
export function webCryptoIdentity(length) {
    try {
        if (!hostGlobal().crypto) {
            return Tag("HostApiUnavailable", { api: "Crypto" });
        }
        return Tag("Generated", identitySecretKey(webCryptoBytes(length), length));
    }
    catch (error) {
        return Tag("EntropySourceFailed", { detail: describeHostError(error) });
    }
}
export async function loadOrCreateBleIdentity(store) {
    let loaded;
    try {
        loaded = await store.load(BLE_IDENTITY_LENGTH);
    }
    catch (error) {
        return Tag("StableIdentityUnavailable", {
            interface: "bluetooth",
            detail: `load Bluetooth LE identity: ${describeHostError(error)}`,
        });
    }
    if (loaded.tag === "Loaded") {
        const validated = bleIdentity(loaded.data);
        return validated.tag === "ValidBleIdentity"
            ? Tag("Available", validated.data)
            : Tag("StableIdentityUnavailable", {
                interface: "bluetooth",
                detail: `stored Bluetooth LE identity has ${validated.data.actualLength} bytes; expected ${BLE_IDENTITY_LENGTH}`,
            });
    }
    if (loaded.tag !== "Missing") {
        return Tag("StableIdentityUnavailable", {
            interface: "bluetooth",
            detail: describeStableIdentityStoreFailure(loaded),
        });
    }
    let generatedBytes;
    try {
        generatedBytes = webCryptoBytes(BLE_IDENTITY_LENGTH);
    }
    catch (error) {
        return Tag("StableIdentityUnavailable", {
            interface: "bluetooth",
            detail: `generate Bluetooth LE identity: ${describeHostError(error)}`,
        });
    }
    const validated = bleIdentity(generatedBytes);
    if (validated.tag !== "ValidBleIdentity") {
        return Tag("StableIdentityUnavailable", {
            interface: "bluetooth",
            detail: `generated Bluetooth LE identity has ${validated.data.actualLength} bytes; expected ${BLE_IDENTITY_LENGTH}`,
        });
    }
    const generated = validated.data;
    let saved;
    try {
        saved = await store.save(generated);
    }
    catch (error) {
        return Tag("StableIdentityUnavailable", {
            interface: "bluetooth",
            detail: `save Bluetooth LE identity: ${describeHostError(error)}`,
        });
    }
    if (saved.tag !== "Saved") {
        return Tag("StableIdentityUnavailable", {
            interface: "bluetooth",
            detail: describeStableIdentityStoreFailure(saved),
        });
    }
    return Tag("Available", generated);
}
export function cooperativeBackendInfo() {
    const webSocketAvailable = typeof globalThis.WebSocket === "function";
    const capabilities = webSocketAvailable
        ? ["WebSocket", "BrowserRendezvous"]
        : [];
    const interfaceKinds = webSocketAvailable
        ? ["WebSocketClient", "BrowserRendezvous"]
        : [];
    return Object.freeze({
        backend: "Cooperative",
        capabilities: Object.freeze(capabilities),
        interfaceKinds: Object.freeze(interfaceKinds),
    });
}
export async function loadBundledWasm() {
    const moduleUrl = bundledWasmModuleUrl();
    try {
        const imported = await import(moduleUrl.href);
        const module = record(imported, "bundled WebAssembly module");
        const initialize = module.default;
        if (typeof initialize !== "function") {
            return Tag("WasmLoadFailed", {
                detail: "bundled WebAssembly module has no initializer",
            });
        }
        await initialize();
        return Tag("Loaded", imported);
    }
    catch (error) {
        return Tag("WasmLoadFailed", { detail: describeHostError(error) });
    }
}
export function bundledWasmModuleUrl() {
    return new URL("../../wasm/prns_wasm.js", import.meta.url);
}
export function browserLimits(limits) {
    return {
        pendingCommands: positiveInteger(limits.pendingCommands, "pending command limit"),
        applicationEvents: positiveInteger(limits.applicationEvents, "application event limit"),
        retainedEventBytes: positiveInteger(limits.retainedEventBytes, "retained event byte limit"),
        diagnostics: positiveInteger(limits.diagnostics, "diagnostic limit"),
    };
}
