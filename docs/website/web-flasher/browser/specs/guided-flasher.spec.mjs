import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

import { installFakeBridge } from "../support/fake-bridge.mjs";

const FIXTURE_MARKER = "PRNS_BROWSER_TEST_FIXTURE_TRUST_ROOT_V1";
const SECRET_SSID = "Victory Local Network";
const SECRET_PASSWORD = "never-send-this-password";

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
      expectedChip: target.expected_chip,
      flashSize: target.flash_size,
      flashMode: target.flash_mode,
      flashFrequency: target.flash_frequency,
      beforeReset: target.before_reset,
      afterReset: target.after_reset,
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
    let resetMode = null;
    class FakeTransport {
      setDeviceLostCallback() {}
      async disconnect() {
        disconnected = true;
      }
    }
    class FakeLoader {
      async main(beforeReset) {
        if (beforeReset !== "default_reset") throw new Error("unexpected before-reset mode");
        return "ESP32-C6";
      }
      async detectFlashSize() {
        return "4MB";
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
      async after(mode) {
        resetMode = mode;
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
        async requestPort() {
          requestedPorts += 1;
          return {};
        },
      },
      TransportImpl: FakeTransport,
      LoaderImpl: FakeLoader,
    });
    return { bundleHash, disconnected, phases, requestedPorts, resetMode, writes };
  }, expectedHash);

  expect(evidence.bundleHash).toBe(expectedHash);
  expect(evidence.requestedPorts).toBe(1);
  expect(evidence.disconnected).toBe(true);
  expect(evidence.resetMode).toBe("hard_reset");
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

test("guided ESP flow verifies the signed candidate, protects credentials, and completes accessibly", async ({
  page,
}) => {
  const evidence = observeCredentialLeaks(page);
  await installFakeBridge(page, { supported: true });
  await selectBoard(page, "heltec-v4");

  await expect(page.getByText("Prepare the board", { exact: true })).toBeVisible();
  await expect(page.getByText(/hold BOOT, tap RESET/i)).toBeVisible();
  await expect(page.getByText(/cannot distinguish Heltec V4 from T-Beam Supreme/i)).toBeVisible();

  const confirmation = page.getByRole("checkbox");
  await confirmation.focus();
  await page.keyboard.press("Space");
  await expect(confirmation).toBeChecked();

  const configure = page.getByRole("radio", { name: "Configure a network locally" });
  await configure.focus();
  await page.keyboard.press("Space");
  await expect(configure).toBeChecked();
  await page.getByLabel("SSID").fill(SECRET_SSID);
  await page.getByLabel("Password").fill(SECRET_PASSWORD);

  const status = page.locator("#flash-status");
  await expect(status).toHaveAttribute("role", "status");
  await expect(status).toHaveAttribute("aria-live", "polite");
  const prepare = page.getByRole("button", { name: "Prepare and verify release" });
  await expect(prepare).toBeEnabled();
  await prepare.click();

  await expect(status).toContainText("Release ready:");
  await expect(status).toBeFocused();
  await expect(page.getByLabel("SSID")).toHaveValue("");
  await expect(page.getByLabel("Password")).toHaveValue("");

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
  await expect(status).toContainText("Verified operation complete");
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

test("browser support is feature-detected and T-Echo stays on the signed UF2 route", async ({
  page,
}) => {
  await installFakeBridge(page, { supported: false });
  await selectBoard(page, "xiao-esp32-c6");

  await expect(page.getByText(/requires a secure current Chrome or Edge browser with Web Serial/i)).toBeVisible();
  await page.getByRole("checkbox").check();
  await expect(page.getByRole("button", { name: "Prepare and verify release" })).toBeDisabled();
  await expect(page.getByText(/cannot distinguish Heltec V4 from T-Beam Supreme/i)).toHaveCount(0);
  expect(await page.evaluate(() => navigator.userAgent.includes("Chrome"))).toBe(true);

  await page.goto("/flash/t-echo");
  await appReady(page);
  await fixtureBuildReady(page);
  await expect(page.getByText(/TECHOBOOT/).first()).toBeVisible();
  await expect(page.getByText(/double-press RESET/i)).toBeVisible();
  await expect(page.locator(".flash-wifi-config")).toHaveCount(0);
  await page.getByRole("checkbox").check();
  await page.getByRole("button", { name: "Prepare and verify release" }).click();
  await expect(page.locator("#flash-status")).toContainText("Release ready:");
  await page.getByRole("button", { name: "Download verified UF2" }).click();
  await expect(page.locator("#flash-status")).toContainText(
    "Verified UF2 downloaded. Copy it to TECHOBOOT",
  );
  await expect(page.getByText(/device-side verification/i)).toHaveCount(0);
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

  expect(await dispatchBeforeUnload(page)).toBe(true);
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

test("cancelling a delayed preparation clears credentials and cannot publish ready", async ({ page }) => {
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
  expect(await page.evaluate(() => window.__prnsFlashTest.state.provisioningWasCleared)).toBe(true);
  held.release();
  await preparationSettled(page);

  await expect(page.locator("#flash-status")).toContainText(/Release preparation cancelled/i);
  await expect(page.getByRole("button", { name: "Connect and flash" })).toBeDisabled();
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
    bridge: await page.evaluate(() => window.__prnsFlashTest.state),
  });
  expect(serialized).not.toContain(SECRET_SSID);
  expect(serialized).not.toContain(SECRET_PASSWORD);
  expect(evidence.pageErrors).toEqual([]);
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
    const ssid = document.querySelector('input[autocomplete="off"]')?.getBoundingClientRect();
    const password = document.querySelector('input[autocomplete="new-password"]')?.getBoundingClientRect();
    if (!ssid || !password) throw new Error("responsive Wi-Fi inputs are missing");
    return { ssidY: ssid.y, passwordY: password.y };
  });
}
