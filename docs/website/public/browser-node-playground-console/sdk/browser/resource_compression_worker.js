"use strict";
const scope = globalThis;
let loaded;
scope.addEventListener("message", (event) => {
    void compress(event.data);
});
function compressionModule(url) {
    if (loaded?.url === url) {
        return loaded.module;
    }
    const module = loadCompressionModule(url);
    loaded = { url, module };
    return module;
}
async function loadCompressionModule(url) {
    const imported = await import(url);
    if (typeof imported !== "object" ||
        imported === null ||
        !("default" in imported) ||
        typeof imported.default !== "function" ||
        !("compressResourceCandidate" in imported) ||
        typeof imported.compressResourceCandidate !== "function") {
        throw new TypeError("resource compression module is invalid");
    }
    const module = imported;
    await module.default();
    return module;
}
async function compress(request) {
    try {
        const module = await compressionModule(request.moduleUrl);
        const options = request.packedMetadata === undefined
            ? { payload: new Uint8Array(request.payload) }
            : {
                payload: new Uint8Array(request.payload),
                packedMetadata: new Uint8Array(request.packedMetadata),
            };
        const candidate = module.compressResourceCandidate(options);
        if (candidate === undefined) {
            scope.postMessage({ id: request.id }, []);
            return;
        }
        const transferred = candidate.slice().buffer;
        scope.postMessage({ id: request.id, candidate: transferred }, [transferred]);
    }
    catch (error) {
        scope.postMessage({
            id: request.id,
            error: error instanceof Error ? error.message : String(error),
        }, []);
    }
}
