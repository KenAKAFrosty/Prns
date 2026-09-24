import { expect, test } from "@playwright/test";

import { installFakeBridge } from "../support/fake-bridge.mjs";

test("the signed fixture hydrates and enables the flasher controls", async ({ page }, testInfo) => {
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });

  try {
    await installFakeBridge(page, { supported: true });
    const response = await page.goto("/flash/xiao-esp32-c6");
    expect(response?.status(), "the staged flasher page must be available").toBe(200);
    await expect.poll(
      () => page.evaluate(() => typeof window.hydration_callback),
      { message: "Dioxus must hydrate before the browser scenarios can run" },
    ).toBe("function");
    await expect(page.getByRole("heading", { name: "Flash a Personal Hopspot" })).toBeVisible();
    await expect(
      page.locator('[data-prns-browser-test-fixture="PRNS_BROWSER_TEST_FIXTURE_TRUST_ROOT_V1"]'),
      "the signed browser fixture must be active",
    ).toHaveCount(1);
    const prepare = page.getByRole("button", { name: "Prepare and verify release" });
    await expect(prepare).toBeVisible();
    await expect(prepare).toBeDisabled();
    await page.getByRole("checkbox").check();
    await expect(prepare, "hydrated controls must respond to board confirmation").toBeEnabled();
    expect(errors, "the fixture must load without browser runtime errors").toEqual([]);
  } finally {
    await testInfo.attach("readiness-runtime-errors", {
      body: JSON.stringify(errors, null, 2),
      contentType: "application/json",
    });
  }
});
