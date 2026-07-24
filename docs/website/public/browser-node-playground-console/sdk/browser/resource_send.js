import { Tag, match } from "../casework.js";
export function byteResourceSource(bytes) {
    return {
        totalBytes: bytes.length,
        read: async (dataStart, dataEnd) => bytes.subarray(dataStart, dataEnd),
    };
}
export function blobResourceSource(blob) {
    return {
        totalBytes: blob.size,
        read: async (dataStart, dataEnd) => new Uint8Array(await blob.slice(dataStart, dataEnd).arrayBuffer()),
    };
}
export async function sendResourceFromSource(linkId, source, compression, packedMetadata, driver) {
    if (!Number.isSafeInteger(source.totalBytes) || source.totalBytes < 0) {
        return Tag("Failed", Tag("PayloadTooLarge"));
    }
    if (packedMetadata !== undefined &&
        !Number.isSafeInteger(packedMetadata.length)) {
        return Tag("Failed", Tag("ResourceMetadataTooLarge"));
    }
    const packedMetadataBytes = packedMetadata?.length;
    const firstPlan = plannedResourceSegment(source.totalBytes, 1, packedMetadataBytes, driver);
    if (firstPlan.tag !== "Ready") {
        return Tag("Failed", planFailure(firstPlan));
    }
    const maximumInFlightSegments = Number.isSafeInteger(driver.maximumInFlightSegments) &&
        driver.maximumInFlightSegments > 0
        ? Math.min(2, driver.maximumInFlightSegments)
        : 1;
    const inFlight = [];
    let nextSegment = 1;
    let preparing = prepareResourceSegment(linkId, source, compression, packedMetadata, driver, nextSegment, firstPlan);
    while (preparing !== undefined || inFlight.length > 0) {
        if (preparing !== undefined &&
            inFlight.length < maximumInFlightSegments) {
            const prepared = await preparing;
            if (prepared.tag === "Failed") {
                return Tag("Failed", prepared.data);
            }
            inFlight.push(issueResourceSegment(driver, prepared.data));
            nextSegment += 1;
            preparing =
                nextSegment <= firstPlan.data.totalSegments
                    ? prepareResourceSegment(linkId, source, compression, packedMetadata, driver, nextSegment)
                    : undefined;
            continue;
        }
        const settled = await inFlight.shift();
        if (settled === undefined) {
            return Tag("Failed", Tag("WriteFailed", {
                detail: "resource send lost its in-flight segment",
            }));
        }
        if (settled.tag === "Failed") {
            return settled;
        }
        if (settled.data.tag !== "ResourceSent") {
            return Tag("Failed", Tag("WriteFailed", {
                detail: "resource segment settled with an unexpected outcome",
            }));
        }
    }
    return Tag("Succeeded", Tag("ResourceSent"));
}
async function prepareResourceSegment(linkId, source, compression, packedMetadata, driver, segmentIndex, knownPlan) {
    const packedMetadataBytes = packedMetadata?.length;
    const plan = knownPlan ??
        plannedResourceSegment(source.totalBytes, segmentIndex, packedMetadataBytes, driver);
    if (plan.tag !== "Ready") {
        return Tag("Failed", planFailure(plan));
    }
    let payload;
    try {
        payload = await source.read(plan.data.dataStart, plan.data.dataEnd);
    }
    catch (error) {
        return Tag("Failed", Tag("WriteFailed", { detail: describeResourceError(error) }));
    }
    if (payload.length !== plan.data.dataEnd - plan.data.dataStart) {
        return Tag("Failed", Tag("WriteFailed", {
            detail: "resource source returned an unexpected byte count",
        }));
    }
    const compressedCandidate = await resourceCompressionCandidate(compression, payload, plan.data.segmentIndex === 1 ? packedMetadata : undefined, driver);
    return Tag("Prepared", {
        totalDataBytes: source.totalBytes,
        segmentIndex: plan.data.segmentIndex,
        ...segmentMetadata(plan.data.segmentIndex, packedMetadata),
        linkId,
        payload,
        ...(compressedCandidate === undefined
            ? {}
            : { compressedCandidate }),
    });
}
async function resourceCompressionCandidate(compression, payload, packedMetadata, driver) {
    try {
        return await match(compression, {
            Auto: () => driver.compress(payload, packedMetadata),
            Never: () => Promise.resolve(undefined),
        });
    }
    catch {
        return undefined;
    }
}
function issueResourceSegment(driver, input) {
    try {
        return driver.issue(input).catch((error) => Tag("Failed", Tag("WriteFailed", { detail: describeResourceError(error) })));
    }
    catch (error) {
        return Promise.resolve(Tag("Failed", Tag("WriteFailed", { detail: describeResourceError(error) })));
    }
}
function plannedResourceSegment(totalDataBytes, segmentIndex, packedMetadataBytes, driver) {
    let plan;
    try {
        plan = resourceSegmentPlan(driver.plan(planInput(totalDataBytes, segmentIndex, packedMetadataBytes)));
    }
    catch (error) {
        return Tag("InvalidPlan", {
            detail: describeResourceError(error),
        });
    }
    if (plan.tag !== "Ready") {
        return plan;
    }
    if (plan.data.segmentIndex !== segmentIndex ||
        plan.data.totalDataBytes !== totalDataBytes ||
        segmentIndex > plan.data.totalSegments ||
        plan.data.dataStart > plan.data.dataEnd ||
        plan.data.dataEnd > totalDataBytes ||
        (segmentIndex === 1 && plan.data.dataStart !== 0) ||
        (segmentIndex === plan.data.totalSegments &&
            plan.data.dataEnd !== totalDataBytes)) {
        return Tag("InvalidPlan", {
            detail: "resource segment plan is internally inconsistent",
        });
    }
    return plan;
}
function planInput(totalDataBytes, segmentIndex, packedMetadataBytes) {
    return packedMetadataBytes === undefined
        ? { totalDataBytes, segmentIndex }
        : { totalDataBytes, segmentIndex, packedMetadataBytes };
}
function segmentMetadata(segmentIndex, packedMetadata) {
    if (packedMetadata === undefined) {
        return { metadata: "none" };
    }
    if (segmentIndex === 1) {
        return { metadata: "packed", packedMetadata };
    }
    return {
        metadata: "sentInFirstSegment",
        packedMetadataBytes: packedMetadata.length,
    };
}
function resourceSegmentPlan(raw) {
    if (typeof raw !== "object" || raw === null || !("type" in raw)) {
        return Tag("InvalidPlan", {
            detail: "resource segment plan is not an object",
        });
    }
    if (raw.type === "ready") {
        try {
            return Tag("Ready", {
                totalStreamBytes: safeIntegerField(raw, "totalStreamBytes"),
                segmentIndex: positiveIntegerField(raw, "segmentIndex"),
                totalSegments: positiveIntegerField(raw, "totalSegments"),
                totalDataBytes: safeIntegerField(raw, "totalDataBytes"),
                dataStart: safeIntegerField(raw, "dataStart"),
                dataEnd: safeIntegerField(raw, "dataEnd"),
                streamBytes: safeIntegerField(raw, "streamBytes"),
            });
        }
        catch (error) {
            return Tag("InvalidPlan", {
                detail: describeResourceError(error),
            });
        }
    }
    if (raw.type !== "rejected" || !("cause" in raw)) {
        return Tag("InvalidPlan", {
            detail: "resource segment plan has an unknown outcome",
        });
    }
    if (raw.cause === "metadataTooLarge") {
        return Tag("MetadataTooLarge");
    }
    if (raw.cause === "payloadTooLarge") {
        return Tag("PayloadTooLarge");
    }
    return Tag("InvalidPlan", {
        detail: `resource segment plan was rejected: ${String(raw.cause)}`,
    });
}
function planFailure(plan) {
    return match(plan, {
        MetadataTooLarge: () => Tag("ResourceMetadataTooLarge"),
        PayloadTooLarge: () => Tag("PayloadTooLarge"),
        InvalidPlan: ({ detail }) => Tag("WriteFailed", { detail }),
    });
}
function safeIntegerField(object, key) {
    if (!(key in object)) {
        throw new TypeError(`resource segment plan is missing ${key}`);
    }
    const value = object[key];
    if (typeof value !== "number" ||
        !Number.isSafeInteger(value) ||
        value < 0) {
        throw new TypeError(`resource segment plan ${key} must be a non-negative safe integer`);
    }
    return value;
}
function positiveIntegerField(object, key) {
    const value = safeIntegerField(object, key);
    if (value === 0) {
        throw new TypeError(`resource segment plan ${key} must be positive`);
    }
    return value;
}
function describeResourceError(error) {
    return error instanceof Error ? error.message : String(error);
}
