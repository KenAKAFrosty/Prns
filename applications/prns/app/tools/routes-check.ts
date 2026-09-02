import { readdirSync, readFileSync } from "node:fs";
import { relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { screenCatalog } from "../src/navigation/catalog.ts";

const appRoot = fileURLToPath(new URL("..", import.meta.url));
const routesRoot = resolve(appRoot, "app");
const routeIdPattern = /^\/\/ route-id: ([a-z0-9.-]+)$/mu;

function fail(message: string): never {
  throw new Error(`routes:check: ${message}`);
}

function routeLeaves(directory: string): readonly string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) {
      return routeLeaves(path);
    }
    if (!entry.isFile() || !entry.name.endsWith(".tsx") || entry.name === "_layout.tsx") {
      return [];
    }
    return [relative(appRoot, path)];
  });
}

const ids = new Set<string>();
const paths = new Set<string>();
const expectedFiles = new Map<string, string>();
for (const entry of screenCatalog) {
  if (ids.has(entry.id)) {
    fail(`duplicate screen id ${entry.id}`);
  }
  if (paths.has(entry.path)) {
    fail(`duplicate route path ${entry.path}`);
  }
  ids.add(entry.id);
  paths.add(entry.path);
  for (const file of entry.routeFiles) {
    const prior = expectedFiles.get(file);
    if (prior !== undefined) {
      fail(`${file} is assigned to both ${prior} and ${entry.id}`);
    }
    expectedFiles.set(file, entry.id);
  }
}

if (screenCatalog.length !== 33) {
  fail(`the closed catalog must contain 33 rows, received ${screenCatalog.length}`);
}
const onboarding = screenCatalog.find((entry) => entry.id === "installation.onboarding");
if (onboarding?.routeFiles.length !== 2) {
  fail("installation.onboarding must be the sole two-leaf optional route");
}
for (const entry of screenCatalog) {
  const expectedCount = entry.id === "installation.onboarding" ? 2 : 1;
  if (entry.routeFiles.length !== expectedCount) {
    fail(`${entry.id} must declare ${expectedCount} route leaf file(s)`);
  }
}

const actualFiles = routeLeaves(routesRoot).toSorted();
const infrastructure = new Set(["app/index.tsx", "app/+not-found.tsx"]);
for (const file of actualFiles) {
  if (infrastructure.has(file)) {
    continue;
  }
  const expectedId = expectedFiles.get(file);
  if (expectedId === undefined) {
    fail(`${file} is an unregistered route leaf`);
  }
  const source = readFileSync(resolve(appRoot, file), "utf8");
  const declaredId = routeIdPattern.exec(source)?.[1];
  if (declaredId !== expectedId) {
    fail(`${file} must declare // route-id: ${expectedId}, received ${String(declaredId)}`);
  }
}

for (const [file, id] of expectedFiles) {
  if (!actualFiles.includes(file)) {
    fail(`${file} is missing for ${id}`);
  }
}
for (const file of infrastructure) {
  if (!actualFiles.includes(file)) {
    fail(`${file} is a required bootstrap route`);
  }
}

console.log(
  `routes:check: ${screenCatalog.length} catalog rows map to ${expectedFiles.size} leaves`,
);
