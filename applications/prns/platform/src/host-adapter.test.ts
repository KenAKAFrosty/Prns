import type { HostSnapshot } from "personal-rns/contract";
import {
  liftHostSnapshot,
  lowerHostSnapshot,
  liftSafeUint,
  lowerSafeUint,
} from "personal-rns-expo/contract-adapter";

test("safe integers remain numbers and reject rounding in both directions", () => {
  for (const value of [0n, 1n, (1n << 53n) - 1n]) {
    expect(lowerSafeUint(liftSafeUint(value))).toBe(value);
  }
  for (const value of [-1n, 1n << 53n, (1n << 64n) - 1n]) {
    expect(() => liftSafeUint(value)).toThrow(RangeError);
  }
  for (const value of [-1, 0.5, Number.NaN, Number.POSITIVE_INFINITY, 2 ** 53]) {
    expect(() => lowerSafeUint(value)).toThrow(RangeError);
  }
});

test("canonical host snapshot survives transport with omitted optionals and exact counters", () => {
  const snapshot = {
    revision: (1n << 64n) - 1n,
    backend: {
      backend: "Native",
      capabilities: ["Bluetooth"],
      interfaceKinds: ["AutomaticBluetoothLe"],
    },
    interfaces: [
      {
        interfaceId: new Uint8Array(8).fill(1),
        health: "Connected",
        rxBytes: (1n << 64n) - 1n,
        txBytes: 1n << 53n,
        rxBps: 42,
        routeCount: 1,
        linkCount: 0,
        transportedLinkCount: 0,
      },
    ],
    routes: [
      {
        destination: new Uint8Array(16).fill(2),
        hops: 1,
        interfaceId: new Uint8Array(8).fill(1),
        learnedAtMillis: 10,
        lastRouteActivityAtMillis: 11,
        expiresAtMillis: 100,
      },
    ],
    activeLinkCount: 0,
    destinationIdentities: [
      { destination: new Uint8Array(16).fill(2), identity: new Uint8Array(16).fill(3) },
    ],
    runtime: {
      running: true,
      uptimeMillis: 13,
      interfaceCount: 1,
      onlineInterfaceCount: 1,
      routeCount: 1,
      linkCount: 0,
      transportedLinkCount: 0,
      rxBytes: (1n << 64n) - 1n,
      txBytes: 1n << 53n,
      rxBps: 42,
      txBps: 0,
    },
    persistence: { persistent: true, restored: true, lastFlushCause: "Startup" },
  };
  const transport = lowerHostSnapshot(snapshot as unknown as HostSnapshot);
  // Generated Option readers may represent absence explicitly; the canonical
  // contract omits these properties under exactOptionalPropertyTypes.
  const [hostInterface] = transport.interfaces;
  const [route] = transport.routes;
  if (hostInterface === undefined || route === undefined) throw new Error("Missing fixture data");
  hostInterface.name = undefined;
  route.viaIdentity = undefined;
  transport.persistence.lastFailureDetail = undefined;
  const restored = liftHostSnapshot(transport);
  expect(restored).toStrictEqual(snapshot);
  expect(Object.hasOwn(restored.interfaces[0] ?? {}, "name")).toBe(false);
  expect(Object.hasOwn(restored.routes[0] ?? {}, "viaIdentity")).toBe(false);
  expect(typeof restored.runtime.rxBytes).toBe("bigint");
  expect(typeof restored.runtime.rxBps).toBe("number");
});
