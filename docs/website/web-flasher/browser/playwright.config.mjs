import { defineConfig, devices } from "@playwright/test";
import path from "node:path";
import { fileURLToPath } from "node:url";

const browserDirectory = path.dirname(fileURLToPath(import.meta.url));
const websiteRoot = path.resolve(browserDirectory, "../..");
const browserOutput = path.join(websiteRoot, "target/browser-tests");

export default defineConfig({
  testDir: "./specs",
  fullyParallel: false,
  workers: 1,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI
    ? [["line"], ["html", {
      open: "never",
      outputFolder: path.join(browserOutput, "report"),
    }]]
    : "line",
  timeout: 45_000,
  expect: { timeout: 10_000 },
  outputDir: path.join(browserOutput, "results"),
  use: {
    ...devices["Desktop Chrome"],
    baseURL: "http://127.0.0.1:4173",
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
    video: "retain-on-failure",
  },
  webServer: {
    command: "node web-flasher/browser/support/serve.mjs",
    cwd: websiteRoot,
    port: 4173,
    reuseExistingServer: false,
    timeout: 30_000,
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});
