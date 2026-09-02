import { existsSync } from "node:fs";

const chromiumCandidates = [
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/Applications/Chromium.app/Contents/MacOS/Chromium",
  "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
  "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
  "/snap/bin/chromium",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
  "/usr/bin/google-chrome",
];

export function findChromium() {
  return [process.env.CHROMIUM_PATH, ...chromiumCandidates].find(
    (candidate) => candidate && existsSync(candidate),
  );
}
