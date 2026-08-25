import { Tag, match_into } from "../casework.js";
import { destinationHash } from "../contract.js";
import { bytesField, field, literalField, numberField, record, stringField, } from "./decoding.js";
import { describeHostError } from "./host_errors.js";
import { hostGlobal } from "./host_apis.js";
import { PrnsValidationError, identitySecretKey, nonNegativeInteger, nowMillis, } from "./values.js";
export const BROWSER_PERSISTENCE_VERSION = 1;
export class BrowserLocalStorageIdentityStore {
    #key;
    constructor(key = "prns.identity.v1") {
        this.#key = key;
    }
    async load(expectedLength) {
        let encoded;
        try {
            const storage = hostGlobal().localStorage;
            if (!storage) {
                return Tag("HostApiUnavailable", { api: "LocalStorage" });
            }
            if (!hostGlobal().atob) {
                return Tag("HostApiUnavailable", { api: "Base64Decoder" });
            }
            encoded = storage.getItem(this.#key);
        }
        catch (error) {
            return Tag("IdentityStoreFailed", {
                operation: "Load",
                detail: describeHostError(error),
            });
        }
        if (encoded === null) {
            return Tag("Missing");
        }
        try {
            return Tag("Loaded", identitySecretKey(decodeBase64(encoded), expectedLength));
        }
        catch (error) {
            return Tag("StoredIdentityInvalid", {
                detail: describeHostError(error),
            });
        }
    }
    async save(secretKey) {
        try {
            const storage = hostGlobal().localStorage;
            if (!storage) {
                return Tag("HostApiUnavailable", { api: "LocalStorage" });
            }
            if (!hostGlobal().btoa) {
                return Tag("HostApiUnavailable", { api: "Base64Encoder" });
            }
            storage.setItem(this.#key, encodeBase64(secretKey));
            return Tag("Saved");
        }
        catch (error) {
            return Tag("IdentityStoreFailed", {
                operation: "Save",
                detail: describeHostError(error),
            });
        }
    }
}
export class BrowserLocalStorageBleIdentityStore {
    #key;
    constructor(key = "prns.ble-identity.v1") {
        this.#key = key;
    }
    async load(expectedLength) {
        try {
            const storage = hostGlobal().localStorage;
            if (!storage) {
                return Tag("HostApiUnavailable", { api: "LocalStorage" });
            }
            if (!hostGlobal().atob) {
                return Tag("HostApiUnavailable", { api: "Base64Decoder" });
            }
            const encoded = storage.getItem(this.#key);
            if (encoded === null) {
                return Tag("Missing");
            }
            const bytes = decodeBase64(encoded);
            if (bytes.length !== expectedLength) {
                return Tag("StoredStableIdentityInvalid", {
                    detail: `stored Bluetooth LE identity has ${bytes.length} bytes; expected ${expectedLength}`,
                });
            }
            return Tag("Loaded", bytes);
        }
        catch (error) {
            return Tag("StableIdentityStoreFailed", {
                operation: "Load",
                detail: describeHostError(error),
            });
        }
    }
    async save(identity) {
        try {
            const storage = hostGlobal().localStorage;
            if (!storage) {
                return Tag("HostApiUnavailable", { api: "LocalStorage" });
            }
            if (!hostGlobal().btoa) {
                return Tag("HostApiUnavailable", { api: "Base64Encoder" });
            }
            storage.setItem(this.#key, encodeBase64(identity));
            return Tag("Saved");
        }
        catch (error) {
            return Tag("StableIdentityStoreFailed", {
                operation: "Save",
                detail: describeHostError(error),
            });
        }
    }
}
export class BrowserLocalStoragePersistenceStore {
    #key;
    constructor(key = "prns.state.v1") {
        this.#key = key;
    }
    async load() {
        let encoded;
        try {
            const storage = hostGlobal().localStorage;
            if (!storage) {
                return Tag("HostApiUnavailable", { api: "LocalStorage" });
            }
            if (!hostGlobal().atob) {
                return Tag("HostApiUnavailable", { api: "Base64Decoder" });
            }
            encoded = storage.getItem(this.#key);
        }
        catch (error) {
            return Tag("PersistenceStoreFailed", {
                operation: "Load",
                detail: describeHostError(error),
            });
        }
        if (encoded === null) {
            return Tag("Missing");
        }
        try {
            return Tag("Loaded", decodeBrowserPersistedState(encoded));
        }
        catch (error) {
            return Tag("StoredPersistenceInvalid", {
                detail: describeHostError(error),
            });
        }
    }
    async save(state) {
        try {
            const storage = hostGlobal().localStorage;
            if (!storage) {
                return Tag("HostApiUnavailable", { api: "LocalStorage" });
            }
            if (!hostGlobal().btoa) {
                return Tag("HostApiUnavailable", { api: "Base64Encoder" });
            }
            storage.setItem(this.#key, encodeBrowserPersistedState(state));
            return Tag("Saved");
        }
        catch (error) {
            return Tag("PersistenceStoreFailed", {
                operation: "Save",
                detail: describeHostError(error),
            });
        }
    }
}
export function browserPersistenceStores(root = "prns") {
    const selected = root.trim();
    if (selected.length === 0) {
        throw new PrnsValidationError("invalid-component", "browser persistence root must not be empty");
    }
    return {
        identityStore: new BrowserLocalStorageIdentityStore(`${selected}.identity.v1`),
        bleIdentityStore: new BrowserLocalStorageBleIdentityStore(`${selected}.ble-identity.v1`),
        persistenceStore: new BrowserLocalStoragePersistenceStore(`${selected}.state.v1`),
    };
}
export function parseBrowserPersistedState(value) {
    const object = record(value, "browser persisted state");
    const persistenceVersion = nonNegativeInteger(numberField(object, "persistenceVersion"), "persisted state version");
    if (persistenceVersion !== BROWSER_PERSISTENCE_VERSION) {
        throw new PrnsValidationError("invalid-component", `persisted state version ${persistenceVersion} does not match ${BROWSER_PERSISTENCE_VERSION}`);
    }
    const rawRatchets = field(object, "ratchets");
    if (!Array.isArray(rawRatchets)) {
        throw new PrnsValidationError("invalid-component", "persisted state ratchets must be an array");
    }
    return {
        type: literalField(object, "type", "persistedState"),
        persistenceVersion,
        takenAtMillis: nowMillis(nonNegativeInteger(numberField(object, "takenAtMillis"), "persisted state timestamp")),
        routingTable: new Uint8Array(bytesField(object, "routingTable")),
        tunnels: new Uint8Array(bytesField(object, "tunnels")),
        destinationIdentities: new Uint8Array(bytesField(object, "destinationIdentities")),
        ratchets: rawRatchets.map((raw) => {
            const ratchet = record(raw, "persisted ratchet");
            return {
                destination: destinationHash(bytesField(ratchet, "destination")),
                sealed: new Uint8Array(bytesField(ratchet, "sealed")),
            };
        }),
    };
}
export function parsePersistenceRestoreReport(value) {
    const report = record(value, "persistence restore report");
    return {
        routes: nonNegativeInteger(numberField(report, "routes"), "restored routes"),
        destinationIdentities: nonNegativeInteger(numberField(report, "destinationIdentities"), "restored destination identities"),
        tunnels: nonNegativeInteger(numberField(report, "tunnels"), "restored tunnels"),
        ratchets: nonNegativeInteger(numberField(report, "ratchets"), "restored ratchets"),
        refused: nonNegativeInteger(numberField(report, "refused"), "refused persistence records"),
        dropped: nonNegativeInteger(numberField(report, "dropped"), "dropped persistence records"),
    };
}
export function describeStableIdentityStoreFailure(failure) {
    return match_into().from(failure, {
        HostApiUnavailable: ({ api }) => `${api} is unavailable`,
        StableIdentityStoreFailed: ({ operation, detail }) => `${operation} stable identity: ${detail}`,
        StoredStableIdentityInvalid: ({ detail }) => detail,
    });
}
export function describePersistenceStoreFailure(failure) {
    return match_into().from(failure, {
        HostApiUnavailable: ({ api }) => `${api} is unavailable`,
        PersistenceStoreFailed: ({ operation, detail }) => `${operation} persistence: ${detail}`,
        StoredPersistenceInvalid: ({ detail }) => detail,
    });
}
function encodeBrowserPersistedState(state) {
    const parsed = parseBrowserPersistedState(state);
    return JSON.stringify({
        type: parsed.type,
        persistenceVersion: parsed.persistenceVersion,
        takenAtMillis: parsed.takenAtMillis,
        routingTable: encodeBase64(parsed.routingTable),
        tunnels: encodeBase64(parsed.tunnels),
        destinationIdentities: encodeBase64(parsed.destinationIdentities),
        ratchets: parsed.ratchets.map(({ destination, sealed }) => ({
            destination: encodeBase64(destination),
            sealed: encodeBase64(sealed),
        })),
    });
}
function decodeBrowserPersistedState(encoded) {
    const stored = record(JSON.parse(encoded), "stored browser persistence");
    const rawRatchets = field(stored, "ratchets");
    if (!Array.isArray(rawRatchets)) {
        throw new PrnsValidationError("invalid-component", "stored persistence ratchets must be an array");
    }
    return parseBrowserPersistedState({
        type: stringField(stored, "type"),
        persistenceVersion: numberField(stored, "persistenceVersion"),
        takenAtMillis: numberField(stored, "takenAtMillis"),
        routingTable: decodeBase64(stringField(stored, "routingTable")),
        tunnels: decodeBase64(stringField(stored, "tunnels")),
        destinationIdentities: decodeBase64(stringField(stored, "destinationIdentities")),
        ratchets: rawRatchets.map((raw) => {
            const ratchet = record(raw, "stored persisted ratchet");
            return {
                destination: decodeBase64(stringField(ratchet, "destination")),
                sealed: decodeBase64(stringField(ratchet, "sealed")),
            };
        }),
    });
}
function encodeBase64(bytes) {
    const btoa = hostGlobal().btoa;
    if (!btoa) {
        throw new PrnsValidationError("missing-host-api", "BrowserLocalStorageIdentityStore requires globalThis.btoa");
    }
    let binary = "";
    for (const byte of bytes) {
        binary += String.fromCharCode(byte);
    }
    return btoa(binary);
}
function decodeBase64(encoded) {
    const atob = hostGlobal().atob;
    if (!atob) {
        throw new PrnsValidationError("missing-host-api", "BrowserLocalStorageIdentityStore requires globalThis.atob");
    }
    const binary = atob(encoded);
    const out = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) {
        out[i] = binary.charCodeAt(i);
    }
    return out;
}
