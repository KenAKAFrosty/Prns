import { collectSupportFacts, supportReport } from "../src/runtime/support-report";
import type { PlatformStatus } from "@prns/app-platform";
import type { prns_service_runtime_uniffi as owners } from "@prns/app-bindings";
jest.mock("@prns/app-bindings", () => ({ prns_host_uniffi: { HostPhysicalRole: { Independent: 0, Supervisor: 1, Member: 2 } } }));
const build = { version: "0.1", nativeBuild: "1", label: "test", variant: "dev", platform: "test", api: 3,
  local: "Ready", network: "Ready", localFailure: false, networkFailure: false };
const status: PlatformStatus = { bluetooth: { selected: true, attached: false, authorization: "allowed", availability: "off" }, notifications: "denied" };

test("support report includes allowlisted Bluetooth facts and one snapshot's aggregate counts, never peer fields", async () => {
  const secret = "PRIVATE_VALUE_MUST_NOT_APPEAR";
  const inspection = jest.fn(async () => ({ interfaces: [
    { role: 0, name: secret }, { role: 1, endpoint: secret }, { role: 2, endpoint: secret }, { role: 2, failureDetail: secret }],
    host: { runtime: { onlineInterfaceCount: 1 }, routes: [{ destination: secret }], activeLinkCount: 3, persistence: { lastFailureDetail: secret } } }));
  const owner = { host: () => ({ inspection }) } as unknown as owners.ServiceRuntimeLike;
  const facts = await collectSupportFacts(async () => ({ ...status, secret }), owner, true, new AbortController().signal);
  const report = supportReport(build, facts);
  expect(inspection).toHaveBeenCalledTimes(1);
  expect(report).toContain("Bluetooth selected: yes"); expect(report).toContain("Bluetooth attached: no");
  expect(report).toContain("Physical peer connections: 2"); expect(report).toContain("Known routes: 1"); expect(report).toContain("Active links: 3");
  expect(report).not.toContain(secret);
});

test("unavailable or offline captures are unknown, while an actual empty snapshot reports zero", async () => {
  const host = jest.fn(() => { throw new Error("private file path"); });
  const owner = { host } as unknown as owners.ServiceRuntimeLike;
  const facts = await collectSupportFacts(async () => { throw new Error("private name"); }, owner, false, new AbortController().signal);
  expect(host).not.toHaveBeenCalled();
  const report = supportReport(build, facts);
  expect(report).toContain("Bluetooth selected: unknown"); expect(report).toContain("Known routes: unknown");
  expect(report).not.toContain("private");
  const empty = { host: () => ({ inspection: async () => ({ interfaces: [], host: { runtime: { onlineInterfaceCount: 0 }, routes: [], activeLinkCount: 0 } }) }) } as unknown as owners.ServiceRuntimeLike;
  expect(supportReport(build, await collectSupportFacts(async () => status, empty, true, new AbortController().signal))).toContain("Known routes: 0");
});

test("a fenced current host cannot turn failed inspection into a zero-peer diagnosis", async () => {
  const owner = { host: () => ({ inspection: async () => { throw new Error("fenced private state"); } }) } as unknown as owners.ServiceRuntimeLike;
  const report = supportReport(build, await collectSupportFacts(async () => status, owner, true, new AbortController().signal));
  expect(report).toContain("Physical peer connections: unknown"); expect(report).not.toContain("fenced");
  expect(report).toContain("Bluetooth permission: allowed");
});
