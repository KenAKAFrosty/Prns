import { Tag } from "./sdk/index.js";
const MAX_DETAIL_LENGTH = 480;
const UTF8 = new TextDecoder("utf-8", { fatal: true });
const WEB_BLUETOOTH_UNAVAILABLE = "Web Bluetooth is not exposed by this browser";
const LINUX_CHROMIUM_BLUETOOTH_GUIDANCE = "Linux Chromium keeps Web Bluetooth behind an experimental switch. Enable chrome://flags/#enable-experimental-web-platform-features or relaunch with --enable-experimental-web-platform-features";
export function boundedDetail(detail) {
    return detail.length <= MAX_DETAIL_LENGTH
        ? detail
        : `${detail.slice(0, MAX_DETAIL_LENGTH)}…`;
}
export function describeBluetoothUnavailable(signals) {
    const platform = `${signals.userAgentData?.platform ?? ""} ${signals.platform ?? ""}`
        .toLowerCase();
    const userAgent = signals.userAgent?.toLowerCase() ?? "";
    const brands = signals.userAgentData?.brands
        ?.map(({ brand }) => brand.toLowerCase())
        .join(" ") ?? "";
    const linux = platform.includes("linux") &&
        !userAgent.includes("android") &&
        !userAgent.includes("cros");
    const chromium = brands.includes("chromium") ||
        userAgent.includes("chrome/") ||
        userAgent.includes("chromium/") ||
        userAgent.includes("edg/");
    return linux && chromium
        ? LINUX_CHROMIUM_BLUETOOTH_GUIDANCE
        : WEB_BLUETOOTH_UNAVAILABLE;
}
export function hex(bytes) {
    return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}
export function presentPacketContent(plaintext) {
    if (plaintext.length === 0) {
        return Tag("Empty");
    }
    try {
        return Tag("Text", { value: UTF8.decode(plaintext) });
    }
    catch {
        return Tag("Binary", {
            byteLength: plaintext.length,
            hexadecimal: hex(plaintext),
        });
    }
}
export function formatBitrate(value) {
    if (value === undefined) {
        return "unknown";
    }
    if (value >= 1_000_000) {
        return `${(value / 1_000_000).toFixed(1)} Mbps`;
    }
    if (value >= 1_000) {
        return `${(value / 1_000).toFixed(1)} Kbps`;
    }
    return `${value} bps`;
}
