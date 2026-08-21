import { Tag } from "../casework.js";
export function connectFailure(interfaceName, stage, error) {
    const name = domExceptionName(error);
    if (name === "SecurityError" || name === "NotAllowedError") {
        return Tag("PermissionDenied", {
            interface: interfaceName,
            stage,
            detail: describeHostError(error),
        });
    }
    if (name === "NotFoundError" && stage === "DeviceSelection") {
        return Tag("Cancelled", { interface: interfaceName, stage });
    }
    return Tag("ConnectionFailed", {
        interface: interfaceName,
        stage,
        detail: describeHostError(error),
    });
}
export function describeHostError(error) {
    if (typeof DOMException !== "undefined" && error instanceof DOMException) {
        return `${error.name}: ${error.message}`;
    }
    if (error instanceof Error) {
        return `${error.name}: ${error.message}`;
    }
    return String(error);
}
export function domExceptionName(error) {
    return typeof DOMException !== "undefined" && error instanceof DOMException
        ? error.name
        : undefined;
}
