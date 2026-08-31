import { expect, test } from "@playwright/test";

const CANONICAL_ORIGIN = "https://reticulum.rs";

test("a Dioxus SSG deep link hydrates one shell and keeps route metadata singular", async ({
  page,
}) => {
  const runtimeErrors = observeRuntimeErrors(page);
  const response = await page.goto("/platforms/");

  expect(response?.status()).toBe(200);
  await appHydrated(page);
  await expect(page.getByRole("heading", { level: 1, name: "Where Prns runs" })).toHaveCount(1);
  await expect(page.locator("#main")).toHaveCount(1);
  await expect(page.locator("body > #main > div > header")).toHaveCount(1);
  await expect(page.locator("body > #main > div > footer")).toHaveCount(1);
  await expectIndexedHead(page, {
    title: "Where Prns runs its Reticulum engine",
    description: /One engine, many homes/,
    path: "/platforms",
  });

  await page.evaluate(() => {
    window.__prnsSsgNavigationMarker = true;
  });
  await page.getByRole("link", { name: "Benchmarks", exact: true }).first().click();

  await expect(page).toHaveURL(/\/benchmarks$/);
  await expect(
    page.getByRole("heading", { level: 1, name: "Benchmarked in the open" }),
  ).toHaveCount(1);
  expect(await page.evaluate(() => window.__prnsSsgNavigationMarker)).toBe(true);
  await expect(page.locator("#main")).toHaveCount(1);
  await expectIndexedHead(page, {
    title: "Benchmarked Reticulum performance in the open | Prns",
    description: /Every number comes from published results/,
    path: "/benchmarks",
  });
  expect(runtimeErrors).toEqual([]);
});

test("client navigation replaces noindex metadata with one canonical", async ({ page }) => {
  const runtimeErrors = observeRuntimeErrors(page);
  await page.goto("/flash/bq-nano-g2-ultra/");
  await appHydrated(page);

  await expect(page).toHaveTitle("B&Q Nano G2 Ultra support status | Prns");
  await expect(page.locator('meta[name="description"]')).toHaveAttribute(
    "content",
    /Browser flashing is not publicly available/,
  );
  await expect(page.locator('link[rel="canonical"]')).toHaveCount(0);
  await expect(page.locator('meta[name="robots"]')).toHaveCount(1);
  await expect(page.locator('meta[name="robots"]')).toHaveAttribute("content", "noindex");

  await page.evaluate(() => {
    window.__prnsSsgNavigationMarker = true;
  });
  await page.getByRole("link", { name: "Flash a Hopspot", exact: true }).click();

  await expect(page).toHaveURL(/\/flash$/);
  await expect(
    page.getByRole("heading", { level: 1, name: "Flash a Personal Hopspot" }),
  ).toHaveCount(1);
  expect(await page.evaluate(() => window.__prnsSsgNavigationMarker)).toBe(true);
  await expectIndexedHead(page, {
    title: "Flash a Personal Reticulum Hopspot | Prns",
    description: /Choose your exact board and flash a signed Prns release/,
    path: "/flash",
  });
  expect(runtimeErrors).toEqual([]);
});

async function appHydrated(page) {
  await expect.poll(
    () => page.evaluate(() => typeof window.hydration_callback),
    { message: "Dioxus should install its hydration callback" },
  ).toBe("function");
}

async function expectIndexedHead(page, { title, description, path }) {
  await expect(page).toHaveTitle(title);
  await expect(page.locator('meta[name="description"]')).toHaveCount(1);
  await expect(page.locator('meta[name="description"]')).toHaveAttribute("content", description);
  await expect(page.locator('link[rel="canonical"]')).toHaveCount(1);
  await expect(page.locator('link[rel="canonical"]')).toHaveAttribute(
    "href",
    `${CANONICAL_ORIGIN}${path}`,
  );
  await expect(page.locator('meta[name="robots"]')).toHaveCount(0);
  await expect(page.locator('meta[property="og:title"]')).toHaveCount(1);
  await expect(page.locator('meta[property="og:title"]')).toHaveAttribute("content", title);
}

function observeRuntimeErrors(page) {
  const messages = [];
  page.on("pageerror", (error) => messages.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") messages.push(message.text());
  });
  return messages;
}
