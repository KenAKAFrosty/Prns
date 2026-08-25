import { expect, test } from "@playwright/test";

const BINDING_PATH = "/assets/flasher/nrf-dfu/prns_nrf_dfu_core.js";
const CORE_PATH = "/assets/flasher/nrf-dfu/prns_nrf_dfu_core_bg.wasm";

test("the exact staged Nordic DFU core runs its typed protocol state in Chromium", async ({
  page,
}) => {
  const bindingHash = process.env.PRNS_EXPECTED_NRF_DFU_BINDING_SHA256;
  const coreHash = process.env.PRNS_EXPECTED_NRF_DFU_CORE_SHA256;
  expect(bindingHash).toMatch(/^[0-9a-f]{64}$/);
  expect(coreHash).toMatch(/^[0-9a-f]{64}$/);
  await page.goto("/");

  const evidence = await page.evaluate(
    async ({ expectedBindingHash, expectedCoreHash, bindingPath, corePath }) => {
      async function exactBytes(path, expectedHash) {
        const response = await fetch(path, { cache: "no-store", credentials: "omit" });
        if (!response.ok) throw new Error(`staged Nordic DFU asset is unavailable: ${path}`);
        const bytes = new Uint8Array(await response.arrayBuffer());
        const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
        const actualHash = Array.from(digest, (byte) =>
          byte.toString(16).padStart(2, "0"),
        ).join("");
        if (actualHash !== expectedHash) {
          throw new Error(`staged Nordic DFU asset hash changed: ${path}`);
        }
        return bytes;
      }

      await exactBytes(bindingPath, expectedBindingHash);
      const wasmBytes = await exactBytes(corePath, expectedCoreHash);
      const core = await import(`${bindingPath}?sha256=${expectedBindingHash}`);
      await core.default({ module_or_path: wasmBytes });

      const compatibility = core.NrfDfuCompatibility.notEnforcedApplication(
        0x0052,
        52840,
        Uint16Array.of(0x0123),
        core.NrfDfuBankLayout.Single,
      );
      const session = new core.NrfDfuSession(
        Uint8Array.of(1, 2, 3),
        Uint8Array.of(
          0x52,
          0x00,
          0x68,
          0xce,
          0xff,
          0xff,
          0xff,
          0xff,
          0x01,
          0x00,
          0x23,
          0x01,
          0xad,
          0xad,
        ),
        compatibility,
      );
      let first;
      let awaiting;
      let accepted;
      let second;
      let retryRequired;
      let retry;
      try {
        first = session.nextFrame();
        awaiting = session.pushAcknowledgement(Uint8Array.of(0xc0, 0x10));
        accepted = session.pushAcknowledgement(Uint8Array.of(0x00, 0x00, 0xf0, 0xc0));
        second = session.nextFrame();
        retryRequired = session.pushAcknowledgement(
          Uint8Array.of(0xc0, 0x10, 0x00, 0x00, 0x00, 0xc0),
        );
        let blockedAcknowledgement = false;
        try {
          session.pushAcknowledgement(Uint8Array.of(0xc0));
        } catch (error) {
          blockedAcknowledgement = /must be retransmitted/.test(String(error));
        }
        retry = session.retryFrame();
        return {
          firstAttempt: first.attempt,
          attemptLimit: first.attemptLimit,
          firstFrameBytes: first.bytes.byteLength,
          awaitingKind: awaiting.kind,
          acceptedKind: accepted.kind,
          waitMilliseconds: accepted.waitMilliseconds,
          writtenBytes: accepted.writtenBytes,
          totalBytes: accepted.totalBytes,
          retryKind: retryRequired.kind,
          retryReason: retryRequired.retryReason(),
          blockedAcknowledgement,
          retryAttempt: retry.attempt,
          retryBytesMatch:
            JSON.stringify(Array.from(retry.bytes)) ===
            JSON.stringify(Array.from(second.bytes)),
          kinds: core.NrfDfuAcknowledgementTransitionKind,
          reasons: core.NrfDfuRetryReason,
        };
      } finally {
        retry?.free();
        retryRequired?.free();
        second?.free();
        accepted?.free();
        awaiting?.free();
        first?.free();
        session.free();
        compatibility.free();
      }
    },
    {
      expectedBindingHash: bindingHash,
      expectedCoreHash: coreHash,
      bindingPath: BINDING_PATH,
      corePath: CORE_PATH,
    },
  );

  expect(evidence.firstAttempt).toBe(1);
  expect(evidence.attemptLimit).toBe(3);
  expect(evidence.firstFrameBytes).toBeGreaterThan(0);
  expect(evidence.awaitingKind).toBe(evidence.kinds.AwaitingMore);
  expect(evidence.acceptedKind).toBe(evidence.kinds.FrameAccepted);
  expect(evidence.waitMilliseconds).toBe(500);
  expect(evidence.writtenBytes).toBe(0);
  expect(evidence.totalBytes).toBe(3);
  expect(evidence.retryKind).toBe(evidence.kinds.RetryRequired);
  expect(evidence.retryReason).toBe(evidence.reasons.MalformedAcknowledgement);
  expect(evidence.blockedAcknowledgement).toBe(true);
  expect(evidence.retryAttempt).toBe(2);
  expect(evidence.retryBytesMatch).toBe(true);
});
