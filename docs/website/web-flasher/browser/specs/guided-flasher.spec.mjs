import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

import { installFakeBridge } from "../support/fake-bridge.mjs";

const FIXTURE_MARKER = "PRNS_BROWSER_TEST_FIXTURE_TRUST_ROOT_V1";
const SECRET_SSID = "Victory Local Network";
const SECRET_PASSWORD = "never-send-this-password";
const GUIDED_FAILURE_CASES = [
  {
    code: "permission_denied",
    recovery: /No serial port was selected.*choose its port when you try again/i,
    forbiddenPhases: ["connecting", "writing", "resetting", "success"],
  },
  {
    code: "wrong_chip",
    recovery: /Re-check the printed board label before retrying/i,
    forbiddenPhases: ["writing", "resetting", "success"],
  },
  {
    code: "reset_failure",
    recovery: /Press RESET and check the next boot/i,
    forbiddenPhases: ["success"],
  },
];

test("the exact staged production bundle performs a hardware-free sparse flash", async ({
  page,
}) => {
  const expectedHash = process.env.PRNS_EXPECTED_FLASH_BUNDLE_SHA256;
  expect(expectedHash).toMatch(/^[0-9a-f]{64}$/);
  await page.goto("/flash/xiao-esp32-c6");
  await appReady(page);
  await fixtureBuildReady(page);

  const evidence = await page.evaluate(async (pinnedHash) => {
    const bundleResponse = await fetch("/assets/flasher/prns-flash.js", {
      cache: "no-store",
      credentials: "omit",
    });
    if (!bundleResponse.ok) throw new Error("staged production bundle is unavailable");
    const bundleBytes = new Uint8Array(await bundleResponse.arrayBuffer());
    const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bundleBytes));
    const bundleHash = Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join("");
    if (bundleHash !== pinnedHash) throw new Error("staged production bundle hash changed");

    const production = await import(`/assets/flasher/prns-flash.js?sha256=${pinnedHash}`);
    production.testing.reset();
    const manifest = await fetch("/releases/0.2.6/flash-manifest.json", {
      cache: "no-store",
      credentials: "omit",
    }).then((response) => response.json());
    const target = manifest.targets.find(({ board_slug: slug }) => slug === "xiao-esp32-c6");
    const request = {
      schema: 1,
      boardSlug: target.board_slug,
      displayName: target.display_name,
      transport: target.transport,
      installMode: "preserve-data",
      eraseConfirmed: false,
      expectedChip: target.expected_chip,
      flashSize: target.flash_size,
      flashMode: target.flash_mode,
      flashFrequency: target.flash_frequency,
      beforeReset: target.before_reset,
      afterReset: target.after_reset,
      mountLabel: null,
      uf2Compatibility: null,
      nrfSerialDfu: null,
      serialFilters: [{ usbVendorId: 0x303a }],
      provisioning: null,
      parts: target.parts.map((part) => ({
        ...part,
        url: `/releases/${manifest.release.version}/${part.path}`,
      })),
    };
    const phases = [];
    const writes = [];
    let disconnected = false;
    let requestedPorts = 0;
    let serialPickerOptions;
    const reset = [];
    class FakeTransport {
      setDeviceLostCallback() {}
      async setDTR(state) { reset.push(["dtr", state]); }
      async setRTS(state) { reset.push(["rts", state]); }
      async disconnect() {
        disconnected = true;
      }
    }
    class FakeLoader {
      chip = { CHIP_NAME: "ESP32-C6" };

      constructor({ transport }) {
        this.transport = transport;
      }
      async main(beforeReset) {
        if (beforeReset !== "default_reset") throw new Error("unexpected before-reset mode");
        return "ESP32-C6 (revision v0.1)";
      }
      async readFlashId() {
        return 0x1640ef;
      }
      async writeFlash(options) {
        const bytes = options.fileArray[0].data;
        writes.push({
          address: options.fileArray[0].address,
          compressed: options.compress,
          eraseAll: options.eraseAll,
          md5: options.calculateMD5Hash(bytes),
          size: bytes.byteLength,
        });
        options.reportProgress(0, bytes.byteLength, bytes.byteLength);
      }
    }

    await production.prepare(request, (event) => phases.push(event.phase), {
      loadEsptool: false,
    });
    await production.flash((event) => phases.push(event.phase), {
      environment: {
        isSecureContext: true,
        addEventListener() {},
        removeEventListener() {},
      },
      serial: {
        async requestPort(options) {
          requestedPorts += 1;
          serialPickerOptions = options;
          return {};
        },
      },
      TransportImpl: FakeTransport,
      LoaderImpl: FakeLoader,
      proveReset: async () => { throw new Error("C6 must not require browser reset evidence"); },
      resetSleep: async (milliseconds) => reset.push(["sleep", milliseconds]),
    });
    return { bundleHash, disconnected, phases, requestedPorts, reset, serialPickerOptions, writes };
  }, expectedHash);

  expect(evidence.bundleHash).toBe(expectedHash);
  expect(evidence.requestedPorts).toBe(1);
  expect(evidence.serialPickerOptions).toEqual({ filters: [{ usbVendorId: 0x303a }] });
  expect(evidence.disconnected).toBe(true);
  expect(evidence.reset).toEqual([
    ["dtr", false],
    ["sleep", 100],
    ["rts", true],
    ["dtr", false],
    ["rts", true],
    ["sleep", 100],
    ["rts", false],
  ]);
  expect(evidence.writes).toHaveLength(3);
  expect(evidence.writes.map(({ address }) => address)).toEqual([0, 0x8000, 0x10000]);
  expect(evidence.writes.every(({ compressed, eraseAll }) => compressed && !eraseAll)).toBe(true);
  expect(evidence.writes.every(({ md5 }) => /^[0-9a-f]{32}$/.test(md5))).toBe(true);
  expect(evidence.phases).toEqual(
    expect.arrayContaining([
      "validating_manifest",
      "downloading",
      "verifying_artifacts",
      "ready",
      "requesting_port",
      "connecting",
      "verifying_target",
      "writing",
      "verifying_flash",
      "resetting",
      "success",
    ]),
  );
});

test("the exact staged production bundle traps same-document Back during an active write", async ({
  page,
}) => {
  const expectedHash = process.env.PRNS_EXPECTED_FLASH_BUNDLE_SHA256;
  expect(expectedHash).toMatch(/^[0-9a-f]{64}$/);
  await selectBoard(page, "xiao-esp32-c6");
  expect(await stagedProductionBundleHash(page)).toBe(expectedHash);

  await page.evaluate(async (pinnedHash) => {
    const production = await import(`/assets/flasher/prns-flash.js?history=${pinnedHash}`);
    production.testing.reset();
    const manifest = await fetch("/releases/0.2.6/flash-manifest.json", {
      cache: "no-store",
      credentials: "omit",
    }).then((response) => response.json());
    const target = manifest.targets.find(({ board_slug: slug }) => slug === "xiao-esp32-c6");
    const request = {
      schema: 1,
      boardSlug: target.board_slug,
      displayName: target.display_name,
      transport: target.transport,
      installMode: "preserve-data",
      eraseConfirmed: false,
      expectedChip: target.expected_chip,
      flashSize: target.flash_size,
      flashMode: target.flash_mode,
      flashFrequency: target.flash_frequency,
      beforeReset: target.before_reset,
      afterReset: target.after_reset,
      mountLabel: null,
      uf2Compatibility: null,
      nrfSerialDfu: null,
      serialFilters: [{ usbVendorId: 0x303a }],
      provisioning: null,
      parts: target.parts.map((part) => ({
        ...part,
        url: `/releases/${manifest.release.version}/${part.path}`,
      })),
    };
    const control = {
      done: false,
      error: null,
      phases: [],
      resume: null,
      writeStarted: false,
      writes: 0,
    };
    window.__prnsProductionHistory = control;
    class FakeTransport {
      setDeviceLostCallback() {}
      async setDTR() {}
      async setRTS() {}
      async disconnect() {}
    }
    class PausedLoader {
      chip = { CHIP_NAME: "ESP32-C6" };

      constructor({ transport }) { this.transport = transport; }
      async main() {}
      async readFlashId() { return 0x1640ef; }
      async writeFlash() {
        control.writes += 1;
        if (control.writes === 1) {
          control.writeStarted = true;
          await new Promise((resolve) => { control.resume = resolve; });
          control.resume = null;
        }
      }
    }

    await production.prepare(request, (event) => control.phases.push(event.phase), {
      loadEsptool: false,
    });
    void production.flash((event) => control.phases.push(event.phase), {
      environment: window,
      serial: { requestPort: async () => ({}) },
      TransportImpl: FakeTransport,
      LoaderImpl: PausedLoader,
      proveReset: async () => { throw new Error("C6 must not require browser reset evidence"); },
      resetSleep: async () => {},
    }).then(() => {
      control.done = true;
    }).catch((error) => {
      control.error = String(error?.message ?? error);
      control.done = true;
    });
  }, expectedHash);

  await expect
    .poll(() => page.evaluate(() => window.__prnsProductionHistory.writeStarted))
    .toBe(true);
  const activeUrl = page.url();
  await page.evaluate(() => history.back());
  await expect.poll(() => page.url()).toBe(activeUrl);
  expect(await page.evaluate(() => window.__prnsProductionHistory.done)).toBe(false);
  expect(await page.evaluate(() => window.__prnsProductionHistory.phases)).toContain("writing");

  await page.evaluate(() => window.__prnsProductionHistory.resume());
  await expect
    .poll(() => page.evaluate(() => window.__prnsProductionHistory.done))
    .toBe(true);
  const result = await page.evaluate(() => window.__prnsProductionHistory);
  expect(result.error).toBeNull();
  expect(result.writes).toBe(3);
  expect(result.phases.at(-1)).toBe("success");
  await expect(page).toHaveURL(/\/flash\/xiao-esp32-c6$/);
});

test("the exact staged production bundle rejects a partial artifact before serial access", async ({
  page,
}) => {
  const expectedHash = process.env.PRNS_EXPECTED_FLASH_BUNDLE_SHA256;
  expect(expectedHash).toMatch(/^[0-9a-f]{64}$/);
  const credentialEvidence = observeCredentialLeaks(page);
  await page.addInitScript(() => {
    window.__prnsProductionPortRequests = 0;
    Object.defineProperty(navigator, "serial", {
      configurable: true,
      value: {
        async requestPort() {
          window.__prnsProductionPortRequests += 1;
          return {};
        },
      },
    });
  });
  let tamperedResponses = 0;
  await page.route("**/firmware/hopspot/heltec-v4/0.2.6/bootloader.bin", async (route) => {
    const original = await route.fetch();
    const body = await original.body();
    expect(body.byteLength).toBeGreaterThan(1);
    tamperedResponses += 1;
    await route.fulfill({ response: original, body: body.subarray(0, body.byteLength - 1) });
  });

  await selectBoard(page, "heltec-v4");
  expect(await stagedProductionBundleHash(page)).toBe(expectedHash);
  await page.getByRole("checkbox").check();
  await page.getByRole("radio", { name: "Configure a network locally" }).check();
  await page.getByLabel("SSID").fill(SECRET_SSID);
  await page.getByLabel("Password").fill(SECRET_PASSWORD);
  await page.getByRole("button", { name: "Prepare and verify release" }).click();

  const status = page.locator("#flash-status");
  await expect(status).toContainText(/firmware part has the wrong byte length/i);
  await expect(status).toContainText(/Do not connect the device.*Reload this page/i);
  await expect(status).toBeFocused();
  await expect(page.getByRole("button", { name: "Connect and flash" })).toBeDisabled();
  await expect(page.getByLabel("SSID")).toHaveValue(SECRET_SSID);
  await expect(page.getByLabel("Password")).toHaveValue(SECRET_PASSWORD);
  expect(tamperedResponses).toBe(1);
  expect(await page.evaluate(() => window.__prnsProductionPortRequests)).toBe(0);
  expect(await page.evaluate(() => window.__prnsFlash?.testing.prepared() ?? null)).toBe(null);
  await assertNoCredentialLeak(page, credentialEvidence);
});

test("the exact staged production bundle starts a real verified UF2 download", async ({
  page,
}) => {
  const expectedHash = process.env.PRNS_EXPECTED_FLASH_BUNDLE_SHA256;
  expect(expectedHash).toMatch(/^[0-9a-f]{64}$/);
  await selectBoard(page, "t-echo");
  expect(await stagedProductionBundleHash(page)).toBe(expectedHash);

  await page.getByRole("checkbox").check();
  await selectTechoInfo(page, "7.3.0");
  await page.getByRole("button", { name: "Prepare and verify release" }).click();
  await expect(page.locator("#flash-status")).toContainText("Release ready:");

  const downloadStarted = page.waitForEvent("download");
  await page.getByRole("button", { name: "Download verified UF2" }).click();
  const download = await downloadStarted;
  expect(download.suggestedFilename()).toBe("prns-hopspot-t-echo.uf2");
  expect(await download.failure()).toBeNull();
  await expect(page.locator("#flash-status")).toContainText(
    "Verified UF2 download requested. Check the browser's downloads",
  );
  await expect(page.getByText("Download requested", { exact: true })).toBeVisible();
  await expect(page.getByText("Complete", { exact: true })).toHaveCount(0);
  await expect(page.getByText(/device-side verification/i)).toHaveCount(0);
});

test("guided ESP flow verifies the signed candidate, protects credentials, and completes accessibly", async ({
  page,
}) => {
  const evidence = observeCredentialLeaks(page);
  await installFakeBridge(page, { supported: true });
  await selectBoard(page, "heltec-v4");

  await expect(page.getByText("Prepare the board", { exact: true })).toBeVisible();
  await expect(page.getByText(/hold BOOT, tap RESET/i)).toBeVisible();
  await expect(page.getByText(/cannot distinguish cataloged boards that share that family/i)).toBeVisible();

  const confirmation = page.getByRole("checkbox");
  await confirmation.focus();
  await page.keyboard.press("Space");
  await expect(confirmation).toBeChecked();

  const configure = page.getByRole("radio", { name: "Configure a network locally" });
  await configure.focus();
  await page.keyboard.press("Space");
  await expect(configure).toBeChecked();
  const ssid = page.getByLabel("SSID");
  const password = page.getByLabel("Password");
  await expect(ssid).toHaveAttribute("name", "username");
  await expect(ssid).toHaveAttribute("autocomplete", "username");
  await expect(password).toHaveAttribute("name", "password");
  await expect(password).toHaveAttribute("autocomplete", "current-password");
  await ssid.fill(SECRET_SSID);
  await password.fill(SECRET_PASSWORD);

  const status = page.locator("#flash-status");
  await expect(status).toHaveAttribute("role", "status");
  await expect(status).toHaveAttribute("aria-live", "polite");
  await expect(status).toHaveAttribute("aria-atomic", "true");
  const prepare = page.getByRole("button", { name: "Prepare and verify release" });
  await expect(prepare).toBeEnabled();
  await prepare.click();

  await expect(status).toContainText("Release ready:");
  await expect(status).toBeFocused();
  await expect(ssid).toHaveValue(SECRET_SSID);
  await expect(password).toHaveValue(SECRET_PASSWORD);

  await page.getByText("Verified artifact details", { exact: true }).click();
  const artifactDetails = page.locator(".flash-artifact-panel");
  await expect(artifactDetails.getByText("0.2.6", { exact: true })).toBeVisible();
  await expect(artifactDetails.getByText(/127 bytes/)).toBeVisible();
  await expect(artifactDetails.getByText(/ef65ab68bd8e33ba/)).toBeVisible();

  const accessibility = await new AxeBuilder({ page })
    .include(".flash-flasher-panel")
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
    .analyze();
  expect(accessibility.violations).toEqual([]);

  await page.getByRole("button", { name: "Connect and flash" }).click();
  await expect(status).toContainText("Verified serial flash complete");
  await expect(status).toContainText("You can close this page");
  await expect(status).toBeFocused();

  const bridgeEvidence = await page.evaluate(() => window.__prnsFlashTest.state);
  expect(bridgeEvidence.lastRequest).toMatchObject({
    boardSlug: "heltec-v4",
    expectedChip: "esp32s3",
    provisioningAction: "configure",
    ssidBytes: new TextEncoder().encode(SECRET_SSID).length,
    passwordBytes: new TextEncoder().encode(SECRET_PASSWORD).length,
    partKinds: ["bootloader", "partition-table", "application"],
  });
  expect(bridgeEvidence.provisioningWasCleared).toBe(true);
  expect(bridgeEvidence.eraseCount).toBe(0);
  expect(bridgeEvidence.phaseLog).not.toContain("erasing");
  expect(bridgeEvidence.phaseLog).toEqual(
    expect.arrayContaining([
      "validating_manifest",
      "downloading",
      "verifying_artifacts",
      "ready",
      "requesting_port",
      "connecting",
      "verifying_target",
      "writing",
      "verifying_flash",
      "resetting",
      "success",
    ]),
  );
  expect(bridgeEvidence.cleanupCount).toBe(1);
  const firmwareRequests = evidence.requests.filter(({ url }) =>
    new URL(url).pathname.includes("/firmware/"),
  );
  expect(firmwareRequests).toHaveLength(3);
  for (const request of firmwareRequests) {
    expect(new URL(request.url).origin).toBe("http://127.0.0.1:4173");
  }
  expect(evidence.requests.some(({ url }) => new URL(url).hostname === "reticulum.rs")).toBe(false);
  await page.evaluate(() => console.info("credential-redaction-probe", { nested: { safe: "value" } }));
  await assertNoCredentialLeak(page, evidence);
  expect(evidence.consoleMessages).toContainEqual(
    expect.objectContaining({
      type: "info",
      args: ["credential-redaction-probe", { nested: { safe: "value" } }],
    }),
  );
});

test("fresh blank install requires separate confirmation and emits an erase phase", async ({
  page,
}) => {
  await installFakeBridge(page, { supported: true });
  await selectBoard(page, "heltec-v4");
  await page.getByRole("checkbox", { name: /checked the board label and image/i }).check();
  await page.getByRole("radio", { name: /Fresh install — erase all device data/i }).check();

  await expect(page.getByText(/Node identity, BLE identity, routes, ratchets/i).last()).toBeVisible();
  await expect(page.getByText(/eFuses and the factory MAC are unaffected/i)).toBeVisible();
  await expect(page.getByRole("radio", { name: "Leave Wi-Fi and TCP blank" })).toBeChecked();
  await expect(page.getByRole("radio", { name: /Preserve existing configuration/i })).toHaveCount(0);

  const prepare = page.getByRole("button", { name: "Prepare and verify release" });
  await expect(prepare).toBeDisabled();
  const eraseConfirmation = page.getByRole("checkbox", {
    name: /understand that Fresh install erases all device data/i,
  });
  await eraseConfirmation.check();
  await expect(prepare).toBeEnabled();
  await prepare.click();
  await expect(page.locator("#flash-status")).toContainText("Release ready:");

  const stateAfterPreparation = await page.evaluate(() => window.__prnsFlashTest.state);
  expect(stateAfterPreparation.lastRequest).toMatchObject({
    installMode: "erase-all",
    eraseConfirmed: true,
    provisioningAction: null,
    ssidBytes: 0,
    passwordBytes: 0,
  });

  await page.getByRole("button", { name: "Connect, erase, and install" }).click();
  await expect(page.locator("#flash-status")).toContainText("Verified serial flash complete");
  const state = await page.evaluate(() => window.__prnsFlashTest.state);
  expect(state.eraseCount).toBe(1);
  expect(state.phaseLog.indexOf("verifying_target")).toBeLessThan(
    state.phaseLog.indexOf("erasing"),
  );
  expect(state.phaseLog.indexOf("erasing")).toBeLessThan(state.phaseLog.indexOf("writing"));
  await expect(eraseConfirmation).not.toBeChecked();

  const accessibility = await new AxeBuilder({ page })
    .include(".flash-flasher-panel")
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
    .analyze();
  expect(accessibility.violations).toEqual([]);
});

test("fresh configured values stay local and replace blank provisioning only when selected", async ({
  page,
}) => {
  const evidence = observeCredentialLeaks(page);
  await installFakeBridge(page, { supported: true });
  await selectBoard(page, "heltec-v4");
  await page.getByRole("checkbox", { name: /checked the board label and image/i }).check();
  await page.getByRole("radio", { name: /Fresh install — erase all device data/i }).check();
  await page
    .getByRole("checkbox", { name: /understand that Fresh install erases all device data/i })
    .check();
  await page.getByRole("radio", { name: "Configure new values locally" }).check();
  await page.getByLabel("SSID").fill(SECRET_SSID);
  await page.getByLabel("Password").fill(SECRET_PASSWORD);
  await page.getByRole("button", { name: "Prepare and verify release" }).click();
  await expect(page.locator("#flash-status")).toContainText("Release ready:");

  expect(await page.evaluate(() => window.__prnsFlashTest.state.lastRequest)).toMatchObject({
    installMode: "erase-all",
    eraseConfirmed: true,
    provisioningAction: "configure",
    ssidBytes: new TextEncoder().encode(SECRET_SSID).length,
    passwordBytes: new TextEncoder().encode(SECRET_PASSWORD).length,
  });
  await assertNoCredentialLeak(page, evidence);
});

test("fresh mode and confirmation changes invalidate the prepared plan and restore defaults", async ({
  page,
}) => {
  await installFakeBridge(page, { supported: true });
  await selectBoard(page, "heltec-v4");
  await page.getByRole("checkbox", { name: /checked the board label and image/i }).check();
  await page.getByRole("radio", { name: /Fresh install — erase all device data/i }).check();
  const eraseConfirmation = page.getByRole("checkbox", {
    name: /understand that Fresh install erases all device data/i,
  });
  await eraseConfirmation.check();
  await page.getByRole("button", { name: "Prepare and verify release" }).click();
  const connectFresh = page.getByRole("button", { name: "Connect, erase, and install" });
  await expect(connectFresh).toBeEnabled();

  await eraseConfirmation.uncheck();
  await expect(connectFresh).toBeDisabled();
  await eraseConfirmation.check();
  await expect(connectFresh).toBeDisabled();

  await page
    .getByRole("radio", { name: /Update firmware and preserve device data/i })
    .check();
  await expect(
    page.getByRole("radio", { name: "Preserve existing configuration" }),
  ).toBeChecked();
  await expect(eraseConfirmation).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Connect and flash" })).toBeDisabled();
});

test("fresh erase failure writes nothing and consumes destructive confirmation", async ({
  page,
}) => {
  await installFakeBridge(page, { supported: true, failureCode: "erase_failure" });
  await selectBoard(page, "heltec-v4");
  await page.getByRole("checkbox", { name: /checked the board label and image/i }).check();
  await page.getByRole("radio", { name: /Fresh install — erase all device data/i }).check();
  const eraseConfirmation = page.getByRole("checkbox", {
    name: /understand that Fresh install erases all device data/i,
  });
  await eraseConfirmation.check();
  await page.getByRole("button", { name: "Prepare and verify release" }).click();
  await page.getByRole("button", { name: "Connect, erase, and install" }).click();

  await expect(page.locator("#flash-status")).toContainText(/device may be blank/i);
  const state = await page.evaluate(() => window.__prnsFlashTest.state);
  expect(state.eraseCount).toBe(1);
  expect(state.completedPartCount).toBe(0);
  expect(state.phaseLog).not.toContain("writing");
  await expect(eraseConfirmation).not.toBeChecked();
  await expect(page.getByRole("button", { name: "Prepare and verify release" })).toBeDisabled();
});

test("fresh device-picker retry requires confirmation again before any erase", async ({ page }) => {
  await installFakeBridge(page, { supported: true, failureCode: "permission_denied" });
  await selectBoard(page, "heltec-v4");
  await page.getByRole("checkbox", { name: /checked the board label and image/i }).check();
  await page.getByRole("radio", { name: /Fresh install — erase all device data/i }).check();
  const eraseConfirmation = page.getByRole("checkbox", {
    name: /understand that Fresh install erases all device data/i,
  });
  await eraseConfirmation.check();
  await page.getByRole("button", { name: "Prepare and verify release" }).click();
  await page.getByRole("button", { name: "Connect, erase, and install" }).click();

  await expect(page.locator("#flash-status")).toContainText(/No serial port was selected/i);
  expect(await page.evaluate(() => window.__prnsFlashTest.state.eraseCount)).toBe(0);
  await expect(eraseConfirmation).not.toBeChecked();
  await expect(page.getByRole("button", { name: "Connect, erase, and install" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Prepare and verify release" })).toBeDisabled();
});

test("fresh install blocks navigation and cancellation after erasure begins", async ({ page }) => {
  await installFakeBridge(page, { supported: true, pauseAtWriting: true });
  await selectBoard(page, "heltec-v4");
  await page.getByRole("checkbox", { name: /checked the board label and image/i }).check();
  await page.getByRole("radio", { name: /Fresh install — erase all device data/i }).check();
  await page
    .getByRole("checkbox", { name: /understand that Fresh install erases all device data/i })
    .check();
  await page.getByRole("button", { name: "Prepare and verify release" }).click();
  await page.getByRole("button", { name: "Connect, erase, and install" }).click();

  const cancellation = page.getByRole("button", {
    name: "Cancellation unavailable after erase begins",
  });
  await expect(cancellation).toBeDisabled();
  expect(await dispatchBeforeUnload(page)).toBe(true);
  expect(await page.evaluate(() => window.__prnsFlashTest.state.cancellationLocked)).toBe(true);
  await page.evaluate(() => window.__prnsFlashTest.resume());
  await expect(page.locator("#flash-status")).toContainText("Verified serial flash complete");
});

test("browser support is feature-detected and T-Echo stays on the signed UF2 route", async ({
  page,
}) => {
  await installFakeBridge(page, { supported: false });
  await selectBoard(page, "xiao-esp32-c6");

  await expect(page.locator("#flash-status")).toContainText(/Web Serial is unavailable/i);
  await expect(page.getByText(/requires a secure current desktop browser with Web Serial/i)).toBeVisible();
  await page.getByRole("checkbox").check();
  await expect(page.getByRole("button", { name: "Prepare and verify release" })).toBeDisabled();
  await expect(page.getByText(/cannot distinguish cataloged boards that share that family/i)).toHaveCount(0);
  expect(await page.evaluate(() => navigator.userAgent.includes("Chrome"))).toBe(true);

  await page.goto("/flash/t-echo");
  await appReady(page);
  await fixtureBuildReady(page);
  await expect(page.locator("#flash-status")).toContainText(/verified UF2 download/i);
  await expect(page.getByText(/TECHOBOOT/).first()).toBeVisible();
  await expect(page.getByText(/double-press RESET/i)).toBeVisible();
  await expect(page.getByRole("group", { name: "Wi-Fi configuration" })).toHaveCount(0);
  await page.getByRole("checkbox").check();
  await selectTechoInfo(page, "6.1.1");
  await page.getByRole("button", { name: "Prepare and verify release" }).click();
  await expect(page.locator("#flash-status")).toContainText("Release ready:");
  expect(await page.evaluate(() => window.__prnsFlashTest.state.lastRequest.partKinds)).toEqual(["uf2"]);
  expect(await page.evaluate(() => window.__prnsFlashTest.state.lastRequest.boardSlug)).toBe("t-echo");
  expect(await page.evaluate(() => window.__prnsFlashTest.state.lastRequest.partPaths)).toEqual([
    "firmware/hopspot/t-echo/0.2.6/t-echo-s140-6.1.1.uf2",
  ]);
  expect(await page.evaluate(() => window.__prnsFlashTest.state.lastRequest.softdeviceVersion)).toBe("6.1.1");
  await page.getByRole("button", { name: "Download verified UF2" }).click();
  await expect(page.locator("#flash-status")).toContainText(
    "Verified UF2 download requested. Check the browser's downloads",
  );
  await expect(page.getByText(/device-side verification/i)).toHaveCount(0);
});

for (const androidPlatform of ["client-hints", "legacy-ua"]) {
  test(`Android ${androidPlatform} Web Serial explains the wired dead end and stays fail closed`, async ({
    page,
  }) => {
    await installFakeBridge(page, { supported: true, androidPlatform });
    await selectBoard(page, "xiao-esp32-c6");

    await expect(page.locator("#flash-status")).toContainText(
      /Bluetooth serial devices only.*never appears in the port picker/i,
    );
    await expect(page.getByText(/only a limited set of devices provides/i)).toBeVisible();
    await page.getByRole("checkbox").check();
    await expect(page.getByRole("button", { name: "Prepare and verify release" })).toBeDisabled();
    await expect(page.getByRole("button", { name: "Connect and flash" })).toBeDisabled();
    expect(await page.evaluate(() => window.__prnsFlashTest.state.readyCount)).toBe(0);

    await page.goto("/flash/t-echo");
    await appReady(page);
    await fixtureBuildReady(page);
    await page.getByRole("checkbox").check();
    await selectTechoInfo(page, "7.3.0");
    await expect(page.getByRole("button", { name: "Prepare and verify release" })).toBeEnabled();
  });
}

test("a Web Serial detection failure keeps ESP preparation and connection fail closed", async ({
  page,
}) => {
  await installFakeBridge(page, { supported: true, supportDetectionFailure: true });
  await selectBoard(page, "xiao-esp32-c6");

  const status = page.locator("#flash-status");
  await expect(status).toHaveAttribute("aria-live", "polite");
  await expect(status).toHaveAttribute("aria-atomic", "true");
  await expect(status).toContainText(/Web Serial is unavailable.*Chrome, Edge, or Firefox.*CLI/i);
  await expect(page.getByText(/requires a secure current desktop browser with Web Serial/i)).toBeVisible();
  await page.getByRole("checkbox").check();
  await expect(page.getByRole("button", { name: "Prepare and verify release" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Connect and flash" })).toBeDisabled();
  expect(await page.evaluate(() => window.__prnsFlashTest.state.readyCount)).toBe(0);
});

test("a device failure gives recovery guidance, cleans up, and moves terminal focus", async ({
  page,
}) => {
  const evidence = observeCredentialLeaks(page);
  await installFakeBridge(page, { supported: true, failureCode: "device_lost" });
  await selectBoard(page, "t-beam-supreme");
  await page.getByRole("checkbox").check();
  await page.getByRole("radio", { name: "Configure a network locally" }).check();
  await page.getByLabel("SSID").fill(SECRET_SSID);
  await page.getByLabel("Password").fill(SECRET_PASSWORD);
  await page.getByRole("button", { name: "Prepare and verify release" }).click();
  await expect(page.locator("#flash-status")).toContainText("Release ready:");
  await page.getByRole("button", { name: "Connect and flash" }).click();

  const status = page.locator("#flash-status");
  await expect(status).toContainText(/Re-enter BOOT mode, press RESET, and restart the complete sparse plan/i);
  await expect(status).toBeFocused();
  await expect(page.getByText("Stopped", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => window.__prnsFlashTest.state.cleanupCount)).toBe(1);
  await assertNoCredentialLeak(page, evidence);
});

test("a malformed preparation event clears the hidden verified plan", async ({ page }) => {
  await installFakeBridge(page, { supported: true, preparationProtocolViolation: true });
  await selectBoard(page, "xiao-esp32-c6");
  await page.getByRole("checkbox").check();
  await page.getByRole("button", { name: "Prepare and verify release" }).click();

  const status = page.locator("#flash-status");
  await expect(status).toContainText(/requires complete byte progress/i);
  await expect(status).toContainText(/Reload this page.*prepare and verify.*CLI/i);
  await expect(status).toBeFocused();
  await expect(page.getByRole("button", { name: "Connect and flash" })).toBeDisabled();
  await expect
    .poll(() => page.evaluate(() => window.__prnsFlashTest.state.clearPreparedCount))
    .toBeGreaterThanOrEqual(2);
  const state = await page.evaluate(() => window.__prnsFlashTest.state);
  expect(state.preparedBoardSlug).toBe(null);
  expect(state.readyCount).toBe(0);
  expect(state.resumePreparation).toBe(null);
});

test("a malformed active-write event cancels JavaScript before another part can write", async ({
  page,
}) => {
  await installFakeBridge(page, { supported: true, deviceProtocolViolation: true });
  await selectBoard(page, "xiao-esp32-c6");
  await page.getByRole("checkbox").check();
  await page.getByRole("button", { name: "Prepare and verify release" }).click();
  await expect(page.locator("#flash-status")).toContainText("Release ready:");
  const clearCountBeforeFlash = await page.evaluate(
    () => window.__prnsFlashTest.state.clearPreparedCount,
  );
  await page.getByRole("button", { name: "Connect and flash" }).click();

  const status = page.locator("#flash-status");
  await expect(status).toContainText(/byte progress is outside its declared total/i);
  await expect(status).toContainText(/Do not assume success.*BOOT\/RESET.*restart the complete plan/i);
  await expect(status).toBeFocused();
  await expect
    .poll(() => page.evaluate(() => window.__prnsFlashTest.state.cleanupCount))
    .toBe(1);
  const state = await page.evaluate(() => window.__prnsFlashTest.state);
  expect(state.cancelled).toBe(true);
  expect(state.clearPreparedCount).toBeGreaterThan(clearCountBeforeFlash);
  expect(state.completedPartCount).toBe(0);
  expect(state.preparedBoardSlug).toBe(null);
  expect(state.phaseLog).not.toContain("verifying_flash");
  expect(state.phaseLog).not.toContain("resetting");
  expect(state.phaseLog).not.toContain("success");
});

for (const scenario of GUIDED_FAILURE_CASES) {
  test(`guided ${scenario.code} failure is focused, recoverable, and redacted`, async ({ page }) => {
    const evidence = observeCredentialLeaks(page);
    await installFakeBridge(page, { supported: true, failureCode: scenario.code });
    await selectBoard(page, "heltec-v4");
    await page.getByRole("checkbox").check();
    await page.getByRole("radio", { name: "Configure a network locally" }).check();
    await page.getByLabel("SSID").fill(SECRET_SSID);
    await page.getByLabel("Password").fill(SECRET_PASSWORD);
    await page.getByRole("button", { name: "Prepare and verify release" }).click();
    await expect(page.locator("#flash-status")).toContainText("Release ready:");
    await expect(page.getByLabel("SSID")).toHaveValue(SECRET_SSID);
    await expect(page.getByLabel("Password")).toHaveValue(SECRET_PASSWORD);
    const connect = page.getByRole("button", { name: "Connect and flash" });
    let scrollPosition;
    if (scenario.code === "permission_denied") {
      await page.setViewportSize({ width: 900, height: 400 });
      await connect.scrollIntoViewIfNeeded();
      scrollPosition = await page.evaluate(() => window.scrollY);
    }
    await connect.click();

    const status = page.locator("#flash-status");
    await expect(status).toContainText(scenario.recovery);
    await expect(status).toBeFocused();
    await expect(page.getByText("Stopped", { exact: true })).toBeVisible();
    const state = await page.evaluate(() => window.__prnsFlashTest.state);
    expect(state.phaseLog.at(-1)).toBe("failed");
    expect(state.cleanupCount).toBe(1);
    expect(state.provisioningWasCleared).toBe(true);
    await expect(page.getByLabel("SSID")).toHaveValue(SECRET_SSID);
    await expect(page.getByLabel("Password")).toHaveValue(SECRET_PASSWORD);
    if (scenario.code === "permission_denied") {
      await expect(connect).toBeEnabled();
      expect(state.preparedBoardSlug).toBe("heltec-v4");
      const finalScrollPosition = await page.evaluate(() => window.scrollY);
      expect(Math.abs(finalScrollPosition - scrollPosition)).toBeLessThan(40);
    } else {
      await expect(connect).toBeDisabled();
      expect(state.preparedBoardSlug).toBe(null);
    }
    for (const phase of scenario.forbiddenPhases) {
      expect(state.phaseLog).not.toContain(phase);
    }
    await assertNoCredentialLeak(page, evidence);
  });
}

test("Wi-Fi and TCP values survive local configuration toggles", async ({ page }) => {
  await installFakeBridge(page, { supported: true });
  await selectBoard(page, "heltec-v4");
  await page.getByRole("radio", { name: "Configure a network locally" }).check();
  await page.getByLabel("SSID").fill(SECRET_SSID);
  await page.getByLabel("Password").fill(SECRET_PASSWORD);
  const tcpToggle = page.getByRole("checkbox", {
    name: /Connect one outbound Reticulum TCP client/,
  });
  await tcpToggle.check();
  await page.getByLabel("TCP target").fill("node.example:4242");

  await page.getByRole("radio", { name: "Preserve existing configuration" }).check();
  await page.getByRole("radio", { name: "Configure a network locally" }).check();
  await expect(page.getByLabel("SSID")).toHaveValue(SECRET_SSID);
  await expect(page.getByLabel("Password")).toHaveValue(SECRET_PASSWORD);
  await expect(tcpToggle).not.toBeChecked();
  await tcpToggle.check();
  await expect(page.getByLabel("TCP target")).toHaveValue("node.example:4242");
});

test("active writes warn on navigation and cancel only at the injected safe boundary", async ({
  page,
}) => {
  const evidence = observeCredentialLeaks(page);
  await installFakeBridge(page, { supported: true, pauseAtWriting: true });
  await selectBoard(page, "heltec-v4");
  await page.getByRole("checkbox").check();
  await page.getByRole("radio", { name: "Configure a network locally" }).check();
  await page.getByLabel("SSID").fill(SECRET_SSID);
  await page.getByLabel("Password").fill(SECRET_PASSWORD);
  await page.getByRole("button", { name: "Prepare and verify release" }).click();
  await expect(page.locator("#flash-status")).toContainText("Release ready:");
  await page.getByRole("button", { name: "Connect and flash" }).click();
  await expect(page.locator("#flash-status")).toContainText(/Writing bootloader/i);
  await expect(page.getByRole("alert")).toContainText(/Internal navigation is blocked/i);

  expect(await dispatchBeforeUnload(page)).toBe(true);
  const activeUrl = page.url();
  await page.locator('a[href="/flash/xiao-esp32-c6"]').click();
  expect(page.url()).toBe(activeUrl);
  await expect(page.locator("#flash-status")).toContainText(/Writing bootloader/i);
  await page.getByRole("button", { name: "Cancel safely" }).click();
  const status = page.locator("#flash-status");
  await expect(status).toContainText(/safe part boundary; no success was reported/i);
  await expect(status).toBeFocused();
  await expect
    .poll(() => page.evaluate(() => window.__prnsFlashTest.state.cleanupCount))
    .toBe(1);
  expect(await dispatchBeforeUnload(page)).toBe(false);
  const state = await page.evaluate(() => window.__prnsFlashTest.state);
  expect(state.phaseLog.at(-1)).toBe("cancelled");
  expect(state.cleanupCount).toBe(1);
  await assertNoCredentialLeak(page, evidence);
});

test("changing provisioning invalidates a delayed preparation without publishing ready", async ({
  page,
}) => {
  const evidence = observeCredentialLeaks(page);
  await installFakeBridge(page, { supported: true });
  const held = await holdFirstArtifact(page, "t-beam-supreme");
  await selectBoard(page, "t-beam-supreme");
  await page.getByRole("checkbox").check();
  await page.getByRole("radio", { name: "Configure a network locally" }).check();
  await page.getByLabel("SSID").fill(SECRET_SSID);
  await page.getByLabel("Password").fill(SECRET_PASSWORD);
  await page.getByRole("button", { name: "Prepare and verify release" }).click();
  await held.started;

  await page.getByRole("radio", { name: "Preserve existing configuration" }).check();
  held.release();
  await preparationSettled(page);

  await expect(page.locator("#flash-status")).toContainText(/Configuration choice changed/i);
  await expect(page.getByRole("button", { name: "Connect and flash" })).toBeDisabled();
  const state = await page.evaluate(() => window.__prnsFlashTest.state);
  expect(state.readyCount).toBe(0);
  expect(state.preparedBoardSlug).toBe(null);
  await assertNoCredentialLeak(page, evidence);
});

test("removing board confirmation invalidates a delayed preparation", async ({ page }) => {
  await installFakeBridge(page, { supported: true });
  const held = await holdFirstArtifact(page, "heltec-v4");
  await selectBoard(page, "heltec-v4");
  const confirmation = page.getByRole("checkbox");
  await confirmation.check();
  await page.getByRole("button", { name: "Prepare and verify release" }).click();
  await held.started;

  await confirmation.uncheck();
  held.release();
  await preparationSettled(page);

  await expect(page.locator("#flash-status")).toContainText(/Board confirmation changed/i);
  await expect(page.getByRole("button", { name: "Connect and flash" })).toBeDisabled();
  expect(await page.evaluate(() => window.__prnsFlashTest.state.readyCount)).toBe(0);
});

test("cancelling a delayed preparation clears its transferred request and retains the form", async ({ page }) => {
  const evidence = observeCredentialLeaks(page);
  await installFakeBridge(page, { supported: true });
  const held = await holdFirstArtifact(page, "heltec-v4");
  await selectBoard(page, "heltec-v4");
  await page.getByRole("checkbox").check();
  await page.getByRole("radio", { name: "Configure a network locally" }).check();
  await page.getByLabel("SSID").fill(SECRET_SSID);
  await page.getByLabel("Password").fill(SECRET_PASSWORD);
  await page.getByRole("button", { name: "Prepare and verify release" }).click();
  await held.started;

  await page.getByRole("button", { name: "Cancel safely" }).click();
  await expect(page.locator("#flash-status")).toBeFocused();
  expect(await page.evaluate(() => window.__prnsFlashTest.state.provisioningWasCleared)).toBe(true);
  held.release();
  await preparationSettled(page);

  await expect(page.locator("#flash-status")).toContainText(/Release preparation cancelled/i);
  await expect(page.getByRole("button", { name: "Connect and flash" })).toBeDisabled();
  await expect(page.getByLabel("SSID")).toHaveValue(SECRET_SSID);
  await expect(page.getByLabel("Password")).toHaveValue(SECRET_PASSWORD);
  expect(await page.evaluate(() => window.__prnsFlashTest.state.readyCount)).toBe(0);
  await assertNoCredentialLeak(page, evidence);
});

test("SPA navigation invalidates delayed preparation and clears credentials", async ({ page }) => {
  const evidence = observeCredentialLeaks(page);
  await installFakeBridge(page, { supported: true });
  const held = await holdFirstArtifact(page, "t-beam-supreme");
  await selectBoard(page, "t-beam-supreme");
  await page.getByRole("checkbox").check();
  await page.getByRole("radio", { name: "Configure a network locally" }).check();
  await page.getByLabel("SSID").fill(SECRET_SSID);
  await page.getByLabel("Password").fill(SECRET_PASSWORD);
  await page.getByRole("button", { name: "Prepare and verify release" }).click();
  await held.started;

  await page.locator('a[href="/flash/xiao-esp32-c6"]').click();
  await expect(page).toHaveURL(/\/flash\/xiao-esp32-c6$/);
  await appReady(page);
  expect(await page.evaluate(() => window.__prnsFlashTest.state.provisioningWasCleared)).toBe(true);
  held.release();
  await preparationSettled(page);

  await expect(page.getByRole("button", { name: "Connect and flash" })).toBeDisabled();
  const state = await page.evaluate(() => window.__prnsFlashTest.state);
  expect(state.readyCount).toBe(0);
  expect(state.preparedBoardSlug).toBe(null);
  await assertNoCredentialLeak(page, evidence);
});

test("responsive and reduced-motion layouts remain usable at release breakpoints", async ({
  page,
}) => {
  await installFakeBridge(page, { supported: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await selectBoard(page, "heltec-v4");
  await page.getByRole("checkbox").check();
  await page.getByRole("radio", { name: "Configure a network locally" }).check();

  expect(
    await page.evaluate(() => ({
      documentWidth: document.documentElement.scrollWidth,
      viewportWidth: window.innerWidth,
      reduced: matchMedia("(prefers-reduced-motion: reduce)").matches,
      scrollBehavior: getComputedStyle(document.documentElement).scrollBehavior,
    })),
  ).toMatchObject({ viewportWidth: 390, reduced: true, scrollBehavior: "auto" });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  const mobileInputs = await inputPositions(page);
  expect(mobileInputs.ssidY).toBeLessThan(mobileInputs.passwordY);

  await page.setViewportSize({ width: 900, height: 900 });
  const desktopInputs = await inputPositions(page);
  expect(Math.abs(desktopInputs.ssidY - desktopInputs.passwordY)).toBeLessThan(2);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.getByRole("radio", { name: /Fresh install — erase all device data/i }).check();
  await expect(page.getByText(/eFuses and the factory MAC are unaffected/i)).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});

test("tampering the signed channel fails before the injected bridge is trusted", async ({ page }) => {
  await installFakeBridge(page, { supported: true });
  await page.route("**/releases/channels/stable.json", async (route) => {
    const original = await route.fetch();
    const tampered = (await original.text()).replace('"version": "0.2.6"', '"version": "0.2.7"');
    await route.fulfill({ response: original, body: tampered });
  });
  await selectBoard(page, "xiao-esp32-c6");
  await page.getByRole("checkbox").check();
  await page.getByRole("button", { name: "Prepare and verify release" }).click();

  const status = page.locator("#flash-status");
  await expect(status).toContainText(/Minisign verification failed/i);
  await expect(status).toBeFocused();
  expect(await page.evaluate(() => window.__prnsFlashTest.state.phaseLog)).toEqual([]);
});

test("oversized channel, manifest, and signature responses fail closed before bridge trust", async ({
  page,
}) => {
  await installFakeBridge(page, { supported: true });
  const oversizedBody = "x".repeat(600 * 1024);
  for (const path of [
    "**/releases/channels/stable.json",
    "**/releases/channels/stable.json.minisig",
    "**/releases/0.2.6/flash-manifest.json",
    "**/releases/0.2.6/flash-manifest.json.minisig",
  ]) {
    await page.route(path, async (route) => {
      await route.fulfill({
        status: 200,
        contentType: path.endsWith(".minisig") ? "text/plain" : "application/json",
        body: oversizedBody,
      });
    });
    await selectBoard(page, "xiao-esp32-c6");
    await page.getByRole("checkbox").check();
    await page.getByRole("button", { name: "Prepare and verify release" }).click();

    const status = page.locator("#flash-status");
    await expect(status).toContainText(/exceeds the browser safety limit/i);
    await expect(status).toContainText(/Do not connect a device.*Reload this page.*use the CLI/i);
    await expect(status).toBeFocused();
    expect(await page.evaluate(() => window.__prnsFlashTest.state.phaseLog)).toEqual([]);
    await page.unroute(path);
  }
});

async function selectBoard(page, slug) {
  await page.goto("/flash");
  await appReady(page);
  await page.locator(`a[href="/flash/${slug}"]`).click();
  await expect(page).toHaveURL(new RegExp(`/flash/${slug}$`));
  await appReady(page);
  await fixtureBuildReady(page);
}

async function appReady(page) {
  await expect(page.getByRole("heading", { name: "Flash a Personal Hopspot" })).toBeVisible();
}

async function fixtureBuildReady(page) {
  await expect(
    page.locator(`[data-prns-browser-test-fixture="${FIXTURE_MARKER}"]`),
  ).toHaveCount(1);
}

async function selectTechoInfo(page, softdeviceVersion) {
  await page.locator('input[type="file"]').setInputFiles({
    name: "INFO_UF2.TXT",
    mimeType: "text/plain",
    buffer: Buffer.from(
      `UF2 Bootloader 0.6.1\r\nModel: LilyGo T-Echo\r\nBoard-ID: nRF52840-TEcho-v1\r\nSoftDevice: S140 version ${softdeviceVersion}\r\n`,
    ),
  });
  await expect(page.getByText(new RegExp(`Detected.*S140 ${softdeviceVersion}`))).toBeVisible();
}

function observeCredentialLeaks(page) {
  const requests = [];
  const consoleMessages = [];
  const pageErrors = [];
  const pendingConsole = [];
  page.on("request", (request) => {
    requests.push({
      headers: request.headers(),
      method: request.method(),
      postData: request.postData(),
      url: request.url(),
    });
  });
  page.on("console", (message) => {
    const captured = { args: [], text: message.text(), type: message.type() };
    consoleMessages.push(captured);
    pendingConsole.push(
      Promise.all(
        message.args().map(async (argument) => {
          try {
            return await argument.jsonValue();
          } catch {
            return "[unserializable console argument]";
          }
        }),
      ).then((args) => {
        captured.args = args;
      }),
    );
  });
  page.on("pageerror", (error) => pageErrors.push(error.message));
  return { requests, consoleMessages, pageErrors, pendingConsole };
}

async function assertNoCredentialLeak(page, evidence) {
  await Promise.all(evidence.pendingConsole);
  const serialized = JSON.stringify({
    requests: evidence.requests,
    consoleMessages: evidence.consoleMessages,
    pageErrors: evidence.pageErrors,
    document: await page.locator("html").innerText(),
    bridge: await page.evaluate(() => window.__prnsFlashTest?.state ?? null),
  });
  expect(serialized).not.toContain(SECRET_SSID);
  expect(serialized).not.toContain(SECRET_PASSWORD);
  expect(evidence.pageErrors).toEqual([]);
}

async function stagedProductionBundleHash(page) {
  return page.evaluate(async () => {
    const response = await fetch("/assets/flasher/prns-flash.js", {
      cache: "no-store",
      credentials: "omit",
    });
    if (!response.ok) throw new Error("staged production bundle is unavailable");
    const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", await response.arrayBuffer()));
    return Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join("");
  });
}

async function holdFirstArtifact(page, boardSlug) {
  let release;
  let started;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  const requestStarted = new Promise((resolve) => {
    started = resolve;
  });
  await page.route(`**/firmware/hopspot/${boardSlug}/**/bootloader.bin`, async (route) => {
    started();
    await gate;
    await route.continue();
  });
  return { release, started: requestStarted };
}

async function preparationSettled(page) {
  await expect
    .poll(() => page.evaluate(() => window.__prnsFlashTest.state.preparationSettledCount))
    .toBe(1);
}

async function dispatchBeforeUnload(page) {
  return page.evaluate(() => {
    const event = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(event);
    return event.defaultPrevented;
  });
}

async function inputPositions(page) {
  return page.evaluate(() => {
    const ssid = document.querySelector('input[autocomplete="username"]')?.getBoundingClientRect();
    const password = document.querySelector('input[autocomplete="current-password"]')?.getBoundingClientRect();
    if (!ssid || !password) throw new Error("responsive Wi-Fi inputs are missing");
    return { ssidY: ssid.y, passwordY: password.y };
  });
}
