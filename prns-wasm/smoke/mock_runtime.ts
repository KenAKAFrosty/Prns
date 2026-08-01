import type { PrnsRuntimeBinding } from "../../prns-js/src/browser/index.js";

export class MockRuntimeBase implements PrnsRuntimeBinding {
  registerInterface(
    _options: Parameters<PrnsRuntimeBinding["registerInterface"]>[0],
  ): ReturnType<PrnsRuntimeBinding["registerInterface"]> {
    return unexpectedRuntimeCall("registerInterface");
  }

  removeInterface(
    _options: Parameters<PrnsRuntimeBinding["removeInterface"]>[0],
  ): ReturnType<PrnsRuntimeBinding["removeInterface"]> {
    return unexpectedRuntimeCall("removeInterface");
  }

  bluetoothIdentity(): ReturnType<
    PrnsRuntimeBinding["bluetoothIdentity"]
  > {
    return unexpectedRuntimeCall("bluetoothIdentity");
  }

  registerSingleDestination(
    _options: Parameters<
      PrnsRuntimeBinding["registerSingleDestination"]
    >[0],
  ): ReturnType<PrnsRuntimeBinding["registerSingleDestination"]> {
    return unexpectedRuntimeCall("registerSingleDestination");
  }

  registerNodePage(
    _options: Parameters<PrnsRuntimeBinding["registerNodePage"]>[0],
  ): ReturnType<PrnsRuntimeBinding["registerNodePage"]> {
    return unexpectedRuntimeCall("registerNodePage");
  }

  announce(
    _options: Parameters<PrnsRuntimeBinding["announce"]>[0],
  ): ReturnType<PrnsRuntimeBinding["announce"]> {
    return unexpectedRuntimeCall("announce");
  }

  sendSinglePacket(
    _options: Parameters<PrnsRuntimeBinding["sendSinglePacket"]>[0],
  ): ReturnType<PrnsRuntimeBinding["sendSinglePacket"]> {
    return unexpectedRuntimeCall("sendSinglePacket");
  }

  establishLink(
    _options: Parameters<PrnsRuntimeBinding["establishLink"]>[0],
  ): ReturnType<PrnsRuntimeBinding["establishLink"]> {
    return unexpectedRuntimeCall("establishLink");
  }

  requestPath(
    _options: Parameters<PrnsRuntimeBinding["requestPath"]>[0],
  ): ReturnType<PrnsRuntimeBinding["requestPath"]> {
    return unexpectedRuntimeCall("requestPath");
  }

  identify(
    _options: Parameters<PrnsRuntimeBinding["identify"]>[0],
  ): ReturnType<PrnsRuntimeBinding["identify"]> {
    return unexpectedRuntimeCall("identify");
  }

  sendLinkPacket(
    _options: Parameters<PrnsRuntimeBinding["sendLinkPacket"]>[0],
  ): ReturnType<PrnsRuntimeBinding["sendLinkPacket"]> {
    return unexpectedRuntimeCall("sendLinkPacket");
  }

  request(
    _options: Parameters<PrnsRuntimeBinding["request"]>[0],
  ): ReturnType<PrnsRuntimeBinding["request"]> {
    return unexpectedRuntimeCall("request");
  }

  respond(
    _options: Parameters<PrnsRuntimeBinding["respond"]>[0],
  ): ReturnType<PrnsRuntimeBinding["respond"]> {
    return unexpectedRuntimeCall("respond");
  }

  resourceSegmentPlan(
    _options: Parameters<PrnsRuntimeBinding["resourceSegmentPlan"]>[0],
  ): ReturnType<PrnsRuntimeBinding["resourceSegmentPlan"]> {
    return unexpectedRuntimeCall("resourceSegmentPlan");
  }

  sendResourceSegment(
    _options: Parameters<PrnsRuntimeBinding["sendResourceSegment"]>[0],
  ): ReturnType<PrnsRuntimeBinding["sendResourceSegment"]> {
    return unexpectedRuntimeCall("sendResourceSegment");
  }

  setLinkResourceStrategy(
    _options: Parameters<
      PrnsRuntimeBinding["setLinkResourceStrategy"]
    >[0],
  ): ReturnType<PrnsRuntimeBinding["setLinkResourceStrategy"]> {
    return unexpectedRuntimeCall("setLinkResourceStrategy");
  }

  setDestinationResourceStrategy(
    _options: Parameters<
      PrnsRuntimeBinding["setDestinationResourceStrategy"]
    >[0],
  ): ReturnType<PrnsRuntimeBinding["setDestinationResourceStrategy"]> {
    return unexpectedRuntimeCall("setDestinationResourceStrategy");
  }

  sendChannelMessage(
    _options: Parameters<PrnsRuntimeBinding["sendChannelMessage"]>[0],
  ): ReturnType<PrnsRuntimeBinding["sendChannelMessage"]> {
    return unexpectedRuntimeCall("sendChannelMessage");
  }

  allowRequester(
    _options: Parameters<PrnsRuntimeBinding["allowRequester"]>[0],
  ): ReturnType<PrnsRuntimeBinding["allowRequester"]> {
    return unexpectedRuntimeCall("allowRequester");
  }

  closeLink(
    _options: Parameters<PrnsRuntimeBinding["closeLink"]>[0],
  ): ReturnType<PrnsRuntimeBinding["closeLink"]> {
    return unexpectedRuntimeCall("closeLink");
  }

  ingest(
    _options: Parameters<PrnsRuntimeBinding["ingest"]>[0],
  ): ReturnType<PrnsRuntimeBinding["ingest"]> {
    return unexpectedRuntimeCall("ingest");
  }

  drainEvents(): ReturnType<PrnsRuntimeBinding["drainEvents"]> {
    return unexpectedRuntimeCall("drainEvents");
  }

  drainOutbound(): ReturnType<PrnsRuntimeBinding["drainOutbound"]> {
    return unexpectedRuntimeCall("drainOutbound");
  }

  persistedState(
    _options: Parameters<PrnsRuntimeBinding["persistedState"]>[0],
  ): ReturnType<PrnsRuntimeBinding["persistedState"]> {
    return unexpectedRuntimeCall("persistedState");
  }

  restorePersistedState(
    _options: Parameters<PrnsRuntimeBinding["restorePersistedState"]>[0],
  ): ReturnType<PrnsRuntimeBinding["restorePersistedState"]> {
    return unexpectedRuntimeCall("restorePersistedState");
  }

  snapshot(): ReturnType<PrnsRuntimeBinding["snapshot"]> {
    return unexpectedRuntimeCall("snapshot");
  }
}

function unexpectedRuntimeCall(operation: string): never {
  throw new Error(`unexpected mock runtime call: ${operation}`);
}
