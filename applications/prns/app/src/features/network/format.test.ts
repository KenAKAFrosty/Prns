import type { LocalNetworkRouteSnapshot } from "@prns-internal/expo";
import { formatNetworkDuration, formatRouteExpiry, formatTraffic, shortNetworkId } from "./format";

describe("network display formatting", () => {
  test.each([
    [0n, "<1s"],
    [999n, "<1s"],
    [1_000n, "1s"],
    [59_999n, "59s"],
    [60_000n, "1m"],
    [3_600_000n, "1h 0m"],
    [3_660_000n, "1h 1m"],
    [86_400_000n, "1d 0h"],
    [90_000_000n, "1d 1h"],
    [18_446_744_073_709_551_615n, "213503982334d 14h"],
  ])("formats %s native milliseconds without a JavaScript clock", (millis, expected) => {
    expect(formatNetworkDuration(millis)).toBe(expected);
  });

  test("uses the explicit expiry state, including a zero remaining duration", () => {
    const route = { expired: false, expiresInMillis: 0n } as LocalNetworkRouteSnapshot;
    expect(formatRouteExpiry(route)).toBe("Expires in <1s");
    expect(formatRouteExpiry({ ...route, expired: true })).toBe("Expired");
  });

  test("keeps large counters exact and shortens hashes without converting them to numbers", () => {
    expect(formatTraffic(13n)).toBe("13 B");
    expect(formatTraffic(1024n)).toBe("1 KiB");
    expect(formatTraffic(1_048_576n)).toBe("1 MiB");
    expect(formatTraffic(18_446_744_073_709_551_615n)).toBe("17592186044415 MiB");
    expect(shortNetworkId(new Uint8Array(16).fill(0x01))).toBe("01010101…0101");
    expect(shortNetworkId(new Uint8Array([0x01, 0xfe]))).toBe("01fe");
  });
});
