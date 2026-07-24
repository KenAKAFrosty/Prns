type CompressionModule = {
  readonly default: () => Promise<unknown>;
  readonly compressResourceCandidate: (options: {
    readonly payload: Uint8Array;
    readonly packedMetadata?: Uint8Array;
  }) => Uint8Array | undefined;
};

type CompressionRequest = {
  readonly id: number;
  readonly moduleUrl: string;
  readonly payload: ArrayBuffer;
  readonly packedMetadata?: ArrayBuffer;
};

type CompressionResponse = {
  readonly id: number;
  readonly candidate?: ArrayBuffer;
  readonly error?: string;
};

type WorkerScope = {
  addEventListener(
    type: "message",
    listener: (event: MessageEvent<CompressionRequest>) => void,
  ): void;
  postMessage(message: CompressionResponse, transfer: Transferable[]): void;
};

const scope = globalThis as unknown as WorkerScope;
let loaded:
  | {
      readonly url: string;
      readonly module: Promise<CompressionModule>;
    }
  | undefined;

scope.addEventListener("message", (event) => {
  void compress(event.data);
});

function compressionModule(url: string): Promise<CompressionModule> {
  if (loaded?.url === url) {
    return loaded.module;
  }
  const module = loadCompressionModule(url);
  loaded = { url, module };
  return module;
}

async function loadCompressionModule(url: string): Promise<CompressionModule> {
  const imported: unknown = await import(url);
  if (
    typeof imported !== "object" ||
    imported === null ||
    !("default" in imported) ||
    typeof imported.default !== "function" ||
    !("compressResourceCandidate" in imported) ||
    typeof imported.compressResourceCandidate !== "function"
  ) {
    throw new TypeError("resource compression module is invalid");
  }
  const module = imported as CompressionModule;
  await module.default();
  return module;
}

async function compress(request: CompressionRequest): Promise<void> {
  try {
    const module = await compressionModule(request.moduleUrl);
    const options =
      request.packedMetadata === undefined
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
    const transferred = candidate.slice().buffer as ArrayBuffer;
    scope.postMessage(
      { id: request.id, candidate: transferred },
      [transferred],
    );
  } catch (error) {
    scope.postMessage(
      {
        id: request.id,
        error: error instanceof Error ? error.message : String(error),
      },
      [],
    );
  }
}
