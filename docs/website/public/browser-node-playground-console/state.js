import { Tag, match_into } from "./sdk/index.js";
export function controlAvailability(autoWifi, webSocket, usb, bluetooth, snapshot) {
    return {
        autoWifiStart: autoWifiStartAvailable(autoWifi),
        autoWifiClose: autoWifiCloseAvailable(autoWifi),
        webSocketConnect: webSocketConnectAvailable(webSocket),
        webSocketClose: webSocketCloseAvailable(webSocket),
        usbConnect: usbConnectAvailable(usb),
        usbClose: usbCloseAvailable(usb),
        bluetoothConnect: bluetoothConnectAvailable(bluetooth),
        bluetoothClose: bluetoothCloseAvailable(bluetooth),
        announce: (snapshot?.interfaces.length ?? 0) > 0,
    };
}
export function bluetoothConnectAvailable(state) {
    return match_into().from(state, {
        Waiting: () => false,
        Ready: () => true,
        Unavailable: () => false,
        Connecting: () => false,
        Session: (session) => session.status.tag === "Closed" || session.status.tag === "Failed",
        SessionFailed: () => true,
        Closing: () => false,
        ConnectFailed: () => true,
        Closed: () => true,
        CloseFailed: () => false,
    });
}
function bluetoothCloseAvailable(state) {
    return match_into().from(state, {
        Waiting: () => false,
        Ready: () => false,
        Unavailable: () => false,
        Connecting: () => false,
        Session: (session) => session.status.tag === "Negotiating" || session.status.tag === "Active",
        SessionFailed: () => false,
        Closing: () => false,
        ConnectFailed: () => false,
        Closed: () => false,
        CloseFailed: () => true,
    });
}
export function bluetoothSession(state) {
    return match_into().from(state, {
        Waiting: () => undefined,
        Ready: () => undefined,
        Unavailable: () => undefined,
        Connecting: () => undefined,
        Session: (session) => session,
        SessionFailed: () => undefined,
        Closing: (session) => session,
        ConnectFailed: () => undefined,
        Closed: () => undefined,
        CloseFailed: ({ session }) => session,
    });
}
export function bluetoothClosableSession(state) {
    return match_into().from(state, {
        Waiting: () => undefined,
        Ready: () => undefined,
        Unavailable: () => undefined,
        Connecting: () => undefined,
        Session: (session) => session.status.tag === "Negotiating" || session.status.tag === "Active"
            ? session
            : undefined,
        SessionFailed: () => undefined,
        Closing: () => undefined,
        ConnectFailed: () => undefined,
        Closed: () => undefined,
        CloseFailed: ({ session }) => session,
    });
}
export function observeBluetoothSession(session) {
    return match_into().from(session.status, {
        Negotiating: () => Tag("Live"),
        Active: () => Tag("Live"),
        Closed: () => Tag("Closed"),
        Failed: (failure) => Tag("Failed", failure),
    });
}
export function webSocketConnectAvailable(state) {
    return match_into().from(state, {
        Waiting: () => false,
        Ready: () => true,
        Unavailable: () => false,
        Connecting: () => false,
        Connected: () => false,
        Closing: () => false,
        ConnectFailed: () => true,
        Closed: () => true,
        CloseFailed: () => false,
    });
}
function webSocketCloseAvailable(state) {
    return match_into().from(state, {
        Waiting: () => false,
        Ready: () => false,
        Unavailable: () => false,
        Connecting: () => false,
        Connected: () => true,
        Closing: () => false,
        ConnectFailed: () => false,
        Closed: () => false,
        CloseFailed: () => true,
    });
}
export function sameAutoWifiStatus(left, right) {
    return match_into().from(left, {
        Starting: () => right.tag === "Starting",
        Discovering: ({ attempt }) => right.tag === "Discovering" && attempt === right.data.attempt,
        Active: ({ gateways }) => right.tag === "Active" && sameGateways(gateways, right.data.gateways),
        Unavailable: (failure) => right.tag === "Unavailable" &&
            sameAutoWifiFailure(failure, right.data),
        Closed: () => right.tag === "Closed",
    });
}
function autoWifiStartAvailable(state) {
    return match_into().from(state, {
        Waiting: () => false,
        Ready: () => true,
        Running: () => false,
        Closed: () => true,
    });
}
function autoWifiCloseAvailable(state) {
    return match_into().from(state, {
        Waiting: () => false,
        Ready: () => false,
        Running: () => true,
        Closed: () => false,
    });
}
function usbConnectAvailable(state) {
    return match_into().from(state, {
        Waiting: () => false,
        Ready: () => true,
        Unavailable: () => false,
        Connecting: () => false,
        Connected: () => false,
        Closing: () => false,
        ConnectFailed: () => true,
        Closed: () => true,
        CloseFailed: () => false,
    });
}
function usbCloseAvailable(state) {
    return match_into().from(state, {
        Waiting: () => false,
        Ready: () => false,
        Unavailable: () => false,
        Connecting: () => false,
        Connected: () => true,
        Closing: () => false,
        ConnectFailed: () => false,
        Closed: () => false,
        CloseFailed: () => true,
    });
}
function sameGateways(left, right) {
    if (left.length !== right.length) {
        return false;
    }
    return left.every((gateway, index) => {
        const candidate = right[index];
        return (candidate !== undefined &&
            gateway.id === candidate.id &&
            gateway.url === candidate.url &&
            gateway.localhost === candidate.localhost &&
            sameBytes(gateway.interfaceId, candidate.interfaceId));
    });
}
function sameAutoWifiFailure(left, right) {
    return match_into().from(left, {
        HostApiUnavailable: ({ api }) => right.tag === "HostApiUnavailable" && api === right.data.api,
        PermissionDenied: ({ interface: interfaceName, stage, detail }) => right.tag === "PermissionDenied" &&
            interfaceName === right.data.interface &&
            stage === right.data.stage &&
            detail === right.data.detail,
        AlreadyActive: ({ interface: interfaceName, target }) => right.tag === "AlreadyActive" &&
            interfaceName === right.data.interface &&
            target === right.data.target,
        SelectionIdentityUnavailable: ({ detail }) => right.tag === "SelectionIdentityUnavailable" &&
            detail === right.data.detail,
        DiscoveryFailed: ({ detail }) => right.tag === "DiscoveryFailed" && detail === right.data.detail,
        RuntimeRejected: ({ operation, detail }) => right.tag === "RuntimeRejected" &&
            operation === right.data.operation &&
            detail === right.data.detail,
    });
}
function sameBytes(left, right) {
    if (left.length !== right.length) {
        return false;
    }
    return left.every((byte, index) => byte === right[index]);
}
