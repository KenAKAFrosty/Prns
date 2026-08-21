import { Tag } from "../casework.js";
import { linkId, packetHash } from "../contract.js";
import { bytesField, numberField, optionalBytesField, stringField, } from "./decoding.js";
import { PrnsValidationError, nonNegativeInteger, } from "./values.js";
export function commandFailed(failure) {
    return Tag("Failed", failure);
}
export function parseCommandSettlement(value) {
    const result = stringField(value, "result");
    if (result === "untracked") {
        return undefined;
    }
    if (result === "failed") {
        return commandFailed(parseCommandFailure(value));
    }
    if (result !== "succeeded") {
        throw new PrnsValidationError("invalid-component", `unknown command settlement result ${result}`);
    }
    const kind = stringField(value, "kind");
    if (kind === "Announced") {
        return Tag("Succeeded", Tag("Announced"));
    }
    if (kind === "LinkCloseQueued") {
        return Tag("Succeeded", Tag("LinkCloseQueued"));
    }
    if (kind === "PacketDelivered") {
        const delivered = {
            rttMillis: nonNegativeInteger(numberField(value, "rttMillis"), "rttMillis"),
            evidence: parseDeliveryEvidence(stringField(value, "evidence")),
        };
        const hash = optionalBytesField(value, "packetHash");
        return Tag("Succeeded", Tag("PacketDelivered", hash === undefined
            ? delivered
            : { ...delivered, packetHash: packetHash(hash) }));
    }
    if (kind === "LinkEstablished") {
        return Tag("Succeeded", Tag("LinkEstablished", {
            linkId: linkId(bytesField(value, "linkId")),
            rttMillis: nonNegativeInteger(numberField(value, "rttMillis"), "rttMillis"),
        }));
    }
    if (kind === "PathDiscovered") {
        return Tag("Succeeded", Tag("PathDiscovered", {
            hops: nonNegativeInteger(numberField(value, "hops"), "hops"),
        }));
    }
    if (kind === "Identified") {
        return Tag("Succeeded", Tag("Identified"));
    }
    if (kind === "ResponseSent") {
        return Tag("Succeeded", Tag("ResponseSent", {
            rttMillis: nonNegativeInteger(numberField(value, "rttMillis"), "rttMillis"),
        }));
    }
    if (kind === "ResourceSent") {
        return Tag("Succeeded", Tag("ResourceSent"));
    }
    if (kind === "ResourceStrategySet") {
        return Tag("Succeeded", Tag("ResourceStrategySet"));
    }
    if (kind === "RequesterAllowed") {
        return Tag("Succeeded", Tag("RequesterAllowed"));
    }
    throw new PrnsValidationError("invalid-component", `unknown command outcome ${kind}`);
}
function parseCommandFailure(value) {
    const kind = stringField(value, "kind");
    if (kind === "NodeStopped") {
        return Tag("NodeStopped");
    }
    if (kind === "Busy") {
        return Tag("Busy");
    }
    if (kind === "PayloadTooLarge") {
        return Tag("PayloadTooLarge");
    }
    if (kind === "ResponseTooLarge") {
        return Tag("ResponseTooLarge");
    }
    if (kind === "UnknownDestination") {
        return Tag("UnknownDestination");
    }
    if (kind === "NotSingleDestination") {
        return Tag("NotSingleDestination");
    }
    if (kind === "AnnounceAppDataTooLong") {
        return Tag("AnnounceAppDataTooLong");
    }
    if (kind === "UnknownInterface") {
        return Tag("UnknownInterface");
    }
    if (kind === "NoRouteToDestination") {
        return Tag("NoRouteToDestination");
    }
    if (kind === "NotDirectlyReachable") {
        return Tag("NotDirectlyReachable");
    }
    if (kind === "PacketCulled") {
        return Tag("PacketCulled");
    }
    if (kind === "DeliveryTimedOut") {
        return Tag("DeliveryTimedOut");
    }
    if (kind === "InvalidBitrate") {
        return Tag("InvalidBitrate");
    }
    if (kind === "BindFailed") {
        return Tag("BindFailed", { detail: stringField(value, "detail") });
    }
    if (kind === "WriteFailed") {
        return Tag("WriteFailed", { detail: stringField(value, "detail") });
    }
    if (kind === "UnsupportedByBackend") {
        return Tag("UnsupportedByBackend");
    }
    if (kind === "UnknownLink") {
        return Tag("UnknownLink");
    }
    if (kind === "LinkNotActive") {
        return Tag("LinkNotActive");
    }
    if (kind === "EntropyUnavailable") {
        return Tag("EntropyUnavailable");
    }
    if (kind === "NotLinkInitiator") {
        return Tag("NotLinkInitiator");
    }
    if (kind === "IdentityNotHeld") {
        return Tag("IdentityNotHeld");
    }
    if (kind === "UnknownRequestHandler") {
        return Tag("UnknownRequestHandler");
    }
    if (kind === "RequestPolicyNotAllowList") {
        return Tag("RequestPolicyNotAllowList");
    }
    if (kind === "RequestAllowListFull") {
        return Tag("RequestAllowListFull");
    }
    if (kind === "LinkBusy") {
        return Tag("LinkBusy");
    }
    if (kind === "ResourceTableFull") {
        return Tag("ResourceTableFull");
    }
    if (kind === "ResourceMetadataTooLarge") {
        return Tag("ResourceMetadataTooLarge");
    }
    if (kind === "ResourceRejectedByPeer") {
        return Tag("ResourceRejectedByPeer");
    }
    if (kind === "ResourceSequencingFailed") {
        return Tag("ResourceSequencingFailed");
    }
    if (kind === "ResourcePredecessorFailed") {
        return Tag("ResourcePredecessorFailed");
    }
    if (kind === "ChannelWindowFull") {
        return Tag("ChannelWindowFull");
    }
    if (kind === "ChannelUntrackable") {
        return Tag("ChannelUntrackable");
    }
    if (kind === "InvalidChannelMessageType") {
        return Tag("InvalidChannelMessageType");
    }
    throw new PrnsValidationError("invalid-component", `unknown command failure ${kind}`);
}
function parseDeliveryEvidence(value) {
    if (value === "ExplicitProof" ||
        value === "ImplicitProof" ||
        value === "Response") {
        return value;
    }
    throw new PrnsValidationError("invalid-component", `unknown delivery evidence ${value}`);
}
