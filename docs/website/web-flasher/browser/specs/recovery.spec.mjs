import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

async function installRecoveryDevice(page, status = "ok") {
  await page.addInitScript((responseStatus) => {
    window.recoveryEvidence = [];
    const device = {
      vendorId: 0x1209,
      productId: 0x0001,
      manufacturerName: "Stay Personal",
      productName: "Personal Hopspot (T1000-E)",
      serialNumber: "PERSONAL-RNS-T1000E-HOP",
      configuration: { interfaces: [{ interfaceNumber: 0 }] },
      opened: false,
      async open() { this.opened = true; },
      async claimInterface(number) { window.recoveryEvidence.push({ claim: number }); },
      async controlTransferOut(control, data) {
        window.recoveryEvidence.push({ control, hasData: data !== undefined });
        return { status: responseStatus, bytesWritten: 0 };
      },
      async close() { this.opened = false; window.recoveryEvidence.push({ closed: true }); },
    };
    Object.defineProperty(navigator, "usb", {
      configurable: true,
      value: {
        async requestDevice(options) { window.recoveryEvidence.push({ picker: options }); return device; },
      },
    });
  }, status);
}

test("T1000-E recovery is available without preparing or downloading a release", async ({ page }) => {
  await installRecoveryDevice(page);
  const firmwareRequests = [];
  page.on("request", (request) => {
    if (request.url().includes("/firmware/")) firmwareRequests.push(request.url());
  });
  await page.goto("/flash/t1000-e");
  const recovery = page.getByRole("button", { name: "Restart into recovery", exact: true });
  await expect(recovery).toBeEnabled();
  await recovery.click();
  await expect(page.getByRole("status").filter({ hasText: "Recovery requested" })).toBeVisible();
  expect(await page.evaluate(() => window.recoveryEvidence)).toEqual([
    { picker: { filters: [{ vendorId: 0x1209, productId: 0x0001, serialNumber: "PERSONAL-RNS-T1000E-HOP" }] } },
    { claim: 0 },
    { control: { requestType: "vendor", recipient: "device", request: 0x55, value: 0x5052, index: 0x4e53 }, hasData: false },
    { closed: true },
  ]);
  expect(firmwareRequests).toEqual([]);
  await page.getByText("Install Meshtastic", { exact: true }).click();
  await expect(page.getByRole("link", { name: "Meshtastic erase and install guide" })).toBeVisible();
  await page.getByText("Recover with the device button", { exact: true }).click();
  await expect(page.getByText(/green light alone does not confirm recovery/)).toBeVisible();
  const accessibility = await new AxeBuilder({ page }).include("#flash-recovery").analyze();
  expect(accessibility.violations).toEqual([]);
});

test("unsupported firmware shows the manual recovery path without claiming a drive appeared", async ({ page }) => {
  await installRecoveryDevice(page, "stall");
  await page.goto("/flash/t1000-e");
  await page.getByRole("button", { name: "Restart into recovery", exact: true }).click();
  await expect(page.getByRole("status").filter({ hasText: "Older releases do not support" })).toBeVisible();
  await expect(page.getByText("Recovery requested", { exact: false })).toHaveCount(0);
});
