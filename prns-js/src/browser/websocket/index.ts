import { Tag } from "../../casework.js";
import type {
  InterfaceId,
  InterfaceRoutingPolicy,
  WebSocketFramingSelection,
} from "../../contract.js";
import { byteKey } from "../bytes.js";
import { connectFailure, describeHostError } from "../host_errors.js";
import type { HostApiUnavailable } from "../host_apis.js";
import type {
  AlreadyActive,
  Cancelled,
  ConnectionFailed,
  ConnectTimedOut,
  InterfaceConnectStage,
  InterfaceSession,
  InvalidTarget,
  PermissionDenied,
} from "../interface_contract.js";
import type { InterfaceOutboundHost } from "../outbound.js";
import type {
  EntropyFailure,
  RuntimeRejected,
  WebSocketFramingCodecBinding,
} from "../runtime_contract.js";
import type {
  BitrateBps,
  ChannelTag,
  HardwareMtu,
} from "../values.js";
import { bitrateBps } from "../values.js";
import {
  BrowserWebSocketSession,
  closeBrowserWebSocket,
} from "./session.js";

export type WebSocketSession = InterfaceSession & {
  readonly name: "websocket";
  readonly url: string;
  readonly framing: WebSocketFramingSelection;
  subscribeStatus(changed: (status: InterfaceSession["status"]) => void): () => void;
};

export type WebSocketConnectOptions = {
  readonly framing?: WebSocketFramingSelection;
  readonly protocols?: string | readonly string[];
  readonly channelTag?: ChannelTag;
  readonly bitrateBps?: BitrateBps;
  readonly hardwareMtu?: HardwareMtu;
  readonly routing?: InterfaceRoutingPolicy;
};

export type WebSocketConnectOutcome =
  | Tag<"Connected", WebSocketSession>
  | HostApiUnavailable<"WebSocket">
  | PermissionDenied<"websocket">
  | Cancelled<"websocket">
  | AlreadyActive<"websocket">
  | InvalidTarget<"websocket">
  | ConnectTimedOut<"websocket">
  | ConnectionFailed<"websocket">
  | RuntimeRejected;

export type WebSocketRuntimeRegistration = {
  readonly channelTag: Uint8Array;
  readonly bitrateBps: BitrateBps;
  readonly hardwareMtu: HardwareMtu;
  readonly routing?: InterfaceRoutingPolicy;
};

type WebSocketRegistrationOutcome =
  | Tag<"Registered", InterfaceId>
  | AlreadyActive<"websocket">
  | RuntimeRejected;

type WebSocketDetachOutcome = Tag<"Detached"> | RuntimeRejected;
type WebSocketIngestOutcome =
  | Tag<"Accepted">
  | EntropyFailure
  | RuntimeRejected;
type WebSocketHostOutcome<Outcome> = Outcome | Promise<Outcome>;
export type WebSocketRuntimeHost = InterfaceOutboundHost & {
  runtimeReadiness(): WebSocketHostOutcome<Tag<"Ready"> | RuntimeRejected>;
  webSocketRegister(
    options: WebSocketRuntimeRegistration,
  ): WebSocketHostOutcome<WebSocketRegistrationOutcome>;
  deactivateInterface(id: InterfaceId): WebSocketHostOutcome<WebSocketDetachOutcome>;
  webSocketIngest(
    id: InterfaceId,
    bytes: Uint8Array,
  ): WebSocketHostOutcome<WebSocketIngestOutcome>;
  createWebSocketFramingCodec(
    selection: WebSocketFramingSelection,
  ): WebSocketFramingCodecBinding;
  websocketBitrateBps(): BitrateBps;
  websocketHardwareMtu(): HardwareMtu;
  websocketFrameCap(): number;
};

type WebSocketOpenOutcome =
  | Tag<"Opened", WebSocket>
  | HostApiUnavailable<"WebSocket">
  | PermissionDenied<"websocket">
  | Cancelled<"websocket">
  | ConnectTimedOut<"websocket">
  | ConnectionFailed<"websocket">;

type CanonicalWebSocketOutcome =
  | Tag<"Canonical", string>
  | InvalidTarget<"websocket">;

const CONNECT_TIMEOUT_MS = 10_000;
const LOOPBACK_BITRATE_FACTOR = 2;
const DEFAULT_FRAMING_SELECTION: WebSocketFramingSelection = "Auto";
export const BROWSER_RENDEZVOUS_FRAMING_SELECTION: WebSocketFramingSelection =
  "RawPacket";

export class WebSocketInterface {
  readonly name = "websocket" as const;
  readonly #host: WebSocketRuntimeHost;
  readonly #activeTags = new Set<string>();

  constructor(host: WebSocketRuntimeHost) {
    this.#host = host;
  }

  async connect(
    url: string | URL,
    options: WebSocketConnectOptions = {},
  ): Promise<WebSocketConnectOutcome> {
    const ready = await this.#host.runtimeReadiness();
    if (ready.tag !== "Ready") {
      return ready;
    }
    const canonical = canonicalWebSocketUrl(url);
    if (canonical.tag !== "Canonical") {
      return canonical;
    }
    const target = canonical.data;
    const protocols = normalizedWebSocketProtocols(options.protocols);
    const framing = options.framing ?? DEFAULT_FRAMING_SELECTION;
    let tag: Uint8Array;
    let codec: WebSocketFramingCodecBinding;
    try {
      tag =
        options.channelTag ??
        browserWebSocketChannelTag(target, protocols, framing);
      codec = this.#host.createWebSocketFramingCodec(framing);
    } catch (error) {
      return connectFailure("websocket", "RuntimeRegistration", error);
    }
    const tagKey = byteKey(tag);
    if (this.#activeTags.has(tagKey)) {
      return Tag("AlreadyActive", { interface: "websocket", target });
    }
    this.#activeTags.add(tagKey);

    let socket: WebSocket | undefined;
    let interfaceId: InterfaceId | undefined;
    let stage: InterfaceConnectStage = "TransportOpen";
    try {
      const opened = await openBrowserWebSocket(target, protocols);
      if (opened.tag !== "Opened") {
        this.#activeTags.delete(tagKey);
        return opened;
      }
      socket = opened.data;
      stage = "RuntimeRegistration";
      const registered = await this.#host.webSocketRegister({
        channelTag: tag,
        bitrateBps:
          options.bitrateBps ?? defaultWebSocketBitrate(this.#host, target),
        hardwareMtu:
          options.hardwareMtu ?? this.#host.websocketHardwareMtu(),
        ...(options.routing === undefined ? {} : { routing: options.routing }),
      });
      if (registered.tag !== "Registered") {
        closeBrowserWebSocket(socket);
        this.#activeTags.delete(tagKey);
        return registered;
      }
      interfaceId = registered.data;
      stage = "Handshake";
      const session = new BrowserWebSocketSession(
        this.#host,
        socket,
        interfaceId,
        target,
        this.#host.websocketFrameCap(),
        framing,
        codec,
        () => this.#activeTags.delete(tagKey),
      );
      session.start();
      return Tag("Connected", session);
    } catch (error) {
      if (interfaceId) {
        await this.#host.deactivateInterface(interfaceId);
      }
      closeBrowserWebSocket(socket);
      this.#activeTags.delete(tagKey);
      return connectFailure("websocket", stage, error);
    }
  }
}

function defaultWebSocketBitrate(
  host: WebSocketRuntimeHost,
  target: string,
): BitrateBps {
  return isLoopbackWebSocketUrl(target)
    ? loopbackWebSocketBitrate(host.websocketBitrateBps())
    : host.websocketBitrateBps();
}

export function loopbackWebSocketBitrate(
  ordinaryBitrate: BitrateBps,
): BitrateBps {
  return bitrateBps(Number(ordinaryBitrate) * LOOPBACK_BITRATE_FACTOR);
}

export function isLoopbackWebSocketUrl(value: string): boolean {
  try {
    const hostname = new URL(value).hostname.toLowerCase();
    return hostname === "localhost" ||
      hostname.endsWith(".localhost") ||
      hostname === "[::1]" ||
      hostname.split(".").length === 4 && hostname.split(".")[0] === "127";
  } catch {
    return false;
  }
}

function requireBrowserWebSocket():
  | Tag<"Available", typeof WebSocket>
  | HostApiUnavailable<"WebSocket"> {
  try {
    const WebSocketCtor = globalThis.WebSocket;
    return WebSocketCtor
      ? Tag("Available", WebSocketCtor)
      : Tag("HostApiUnavailable", { api: "WebSocket" });
  } catch {
    return Tag("HostApiUnavailable", { api: "WebSocket" });
  }
}

async function openBrowserWebSocket(
  url: string,
  protocols?: string | readonly string[],
): Promise<WebSocketOpenOutcome> {
  const available = requireBrowserWebSocket();
  if (available.tag !== "Available") {
    return available;
  }
  const protocolList =
    protocols === undefined || typeof protocols === "string"
      ? protocols
      : [...protocols];
  let socket: WebSocket;
  try {
    const WebSocketCtor = available.data;
    socket =
      protocolList === undefined
        ? new WebSocketCtor(url)
        : new WebSocketCtor(url, protocolList);
  } catch (error) {
    return connectFailure("websocket", "TransportOpen", error);
  }
  try {
    socket.binaryType = "arraybuffer";
  } catch (error) {
    closeBrowserWebSocket(socket);
    return connectFailure("websocket", "TransportOpen", error);
  }
  return new Promise((resolve) => {
    let timeout: number | undefined;
    const cleanup = (): void => {
      if (timeout !== undefined) {
        globalThis.clearTimeout(timeout);
      }
      socket.removeEventListener("open", handleOpen);
      socket.removeEventListener("error", handleError);
      socket.removeEventListener("close", handleClose);
    };
    const handleOpen = (): void => {
      cleanup();
      resolve(Tag("Opened", socket));
    };
    const handleError = (): void => {
      cleanup();
      closeBrowserWebSocket(socket);
      resolve(
        Tag("ConnectionFailed", {
          interface: "websocket",
          stage: "TransportOpen",
          detail: `WebSocket connection failed for ${url}`,
        }),
      );
    };
    const handleClose = (): void => {
      cleanup();
      resolve(
        Tag("ConnectionFailed", {
          interface: "websocket",
          stage: "TransportOpen",
          detail: `WebSocket connection closed before opening for ${url}`,
        }),
      );
    };
    const handleTimeout = (): void => {
      cleanup();
      closeBrowserWebSocket(socket);
      resolve(
        Tag("TimedOut", {
          interface: "websocket",
          stage: "TransportOpen",
          timeoutMs: CONNECT_TIMEOUT_MS,
        }),
      );
    };
    try {
      timeout = globalThis.setTimeout(handleTimeout, CONNECT_TIMEOUT_MS);
      socket.addEventListener("open", handleOpen);
      socket.addEventListener("error", handleError);
      socket.addEventListener("close", handleClose);
    } catch (error) {
      cleanup();
      closeBrowserWebSocket(socket);
      resolve(connectFailure("websocket", "TransportOpen", error));
    }
  });
}

function canonicalWebSocketUrl(url: string | URL): CanonicalWebSocketOutcome {
  let target: URL;
  try {
    target = new URL(url.toString());
  } catch (error) {
    return Tag("InvalidTarget", {
      interface: "websocket",
      target: url.toString(),
      detail: describeHostError(error),
    });
  }
  if (target.protocol !== "ws:" && target.protocol !== "wss:") {
    return Tag("InvalidTarget", {
      interface: "websocket",
      target: target.toString(),
      detail: "WebSocket URL must use the ws or wss scheme",
    });
  }
  return Tag("Canonical", target.toString());
}

function normalizedWebSocketProtocols(
  protocols: string | readonly string[] | undefined,
): string | readonly string[] | undefined {
  if (protocols === undefined || typeof protocols === "string") {
    return protocols;
  }
  return protocols.length === 0 ? undefined : [...protocols];
}

function browserWebSocketChannelTag(
  url: string,
  protocols: string | readonly string[] | undefined,
  framing: WebSocketFramingSelection,
): Uint8Array {
  const protocolList =
    protocols === undefined
      ? []
      : typeof protocols === "string"
        ? [protocols]
        : protocols;
  return new TextEncoder().encode(
    JSON.stringify(["websocket-client", url, protocolList, framing]),
  );
}
