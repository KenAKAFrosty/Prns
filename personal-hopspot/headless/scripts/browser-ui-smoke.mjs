// Manual qualification against a locally hosted Hopspot browser bundle.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const [address, expectedEndpoint] = process.argv.slice(2);
if (!address || !expectedEndpoint) {
  throw new Error("Usage: node browser-ui-smoke.mjs HTTP_ORIGIN WEBSOCKET_URL");
}
const origin = new URL(address).origin;
const root = fileURLToPath(new URL("../../../", import.meta.url));
const require = createRequire(resolve(root, "docs/website/package.json"));
const { chromium, expect } = require("@playwright/test");
const browser = await chromium.launch({
  headless: true,
  ...(process.env.PRNS_BROWSER_CHANNEL ? { channel: process.env.PRNS_BROWSER_CHANNEL } : {}),
});
try {
  const context = await browser.newContext();
  const blocked = [];
  const requests = [];
  const errors = [];
  await context.route("**/*", route => {
    const url = route.request().url();
    requests.push(url);
    if (new URL(url).origin !== origin) {
      blocked.push(url);
      return route.abort();
    }
    return route.continue();
  });
  const page = await context.newPage();
  page.on("pageerror", error => errors.push(error.message));
  await page.goto(`${origin}/`);
  await expect(page.locator("#websocket-connect")).toBeEnabled({ timeout: 30000 });
  assert.equal(await page.locator("#websocket-url").inputValue(), expectedEndpoint);
  const destination = (await page.locator("#destination").textContent()).trim();
  assert.match(destination, /^lxmf\.delivery [0-9a-f]{32}$/i);
  await page.locator("#websocket-connect").click();
  await expect(page.locator("#websocket-state")).toHaveText("Active", { timeout: 15000 });
  await page.locator("#websocket-close").click();
  await expect(page.locator("#websocket-state")).toHaveText("Closed", { timeout: 10000 });
  await page.reload();
  await expect(page.locator("#websocket-connect")).toBeEnabled({ timeout: 30000 });
  assert.equal((await page.locator("#destination").textContent()).trim(), destination);
  assert.deepEqual(blocked, []);
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({
    browser: await browser.version(), origin,
    defaultEndpoint: await page.locator("#websocket-url").inputValue(),
    destinationRetained: true, connectClose: true,
    blockedExternalRequests: blocked, pageErrors: errors,
    requestedOrigins: [...new Set(requests.map(url => new URL(url).origin))],
  }, null, 2));
} finally {
  await browser.close();
}
