import { createHash } from "node:crypto";
import { cp, mkdir, readFile, rm } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { safeFixtureFiles } from "./fixture-safety.mjs";

const supportDirectory = path.dirname(fileURLToPath(import.meta.url));
const websiteRoot = path.resolve(supportDirectory, "../../..");
const sourceRoot = path.resolve(
  supportDirectory,
  "../fixtures/signed-candidate",
);
const siteRoot = path.resolve(
  process.argv[2] ?? path.join(websiteRoot, "target/browser-tests/site"),
);
const allowedOutputRoot = path.join(websiteRoot, "target/browser-tests");

if (!isWithin(allowedOutputRoot, siteRoot)) {
  throw new Error(`fixture output must remain under ${allowedOutputRoot}`);
}

const sourceFiles = await safeFixtureFiles(sourceRoot);

const publicKey = await readFile(path.join(sourceRoot, "minisign.pub"), "utf8");
if (/PRIVATE|SECRET/i.test(publicKey)) {
  throw new Error("the browser fixture must contain only a public test key");
}
const keyId = publicKey
  .split("\n", 1)[0]
  .match(/^untrusted comment: minisign public key ([0-9A-F]{16})$/)?.[1];
if (!keyId) {
  throw new Error("the browser fixture public key has no canonical key ID");
}

const channelPath = path.join(sourceRoot, "releases/channels/stable.json");
const channelBytes = await readFile(channelPath);
const channel = JSON.parse(channelBytes);
if (
  channel.schema !== 1 ||
  channel.channel !== "stable" ||
  !/^[A-Za-z0-9.+-]+$/.test(channel.version) ||
  channel.version.toLowerCase() === "next"
) {
  throw new Error("the browser fixture channel identity is invalid");
}
await requireSignature(channelPath);

const expectedManifestUrl = `https://reticulum.rs/releases/${channel.version}/flash-manifest.json`;
if (channel.manifest_url !== expectedManifestUrl) {
  throw new Error("the browser fixture channel does not use an immutable release URL");
}
const manifestPath = path.join(
  sourceRoot,
  "releases",
  channel.version,
  "flash-manifest.json",
);
const manifestBytes = await readFile(manifestPath);
await requireSignature(manifestPath);
if (sha256(manifestBytes) !== channel.manifest_sha256) {
  throw new Error("the signed fixture channel hash does not match its manifest");
}

const manifest = JSON.parse(manifestBytes);
if (
  manifest.schema !== 2 ||
  manifest.release?.version !== channel.version ||
  manifest.release?.channel !== channel.channel ||
  manifest.signing?.key_id !== keyId
) {
  throw new Error("the browser fixture manifest identity is inconsistent");
}
const expectedBoards = [
  "heltec-v4",
  "t-beam-supreme",
  "xiao-esp32-c6",
  "t-echo",
];
const actualBoards = manifest.targets.map((target) => target.board_slug).sort();
if (JSON.stringify(actualBoards) !== JSON.stringify([...expectedBoards].sort())) {
  throw new Error("the browser fixture must contain every shipping board exactly once");
}

const immutableReleaseRoot = path.join(
  sourceRoot,
  "releases",
  channel.version,
);
for (const target of manifest.targets) {
  if (!Array.isArray(target.parts) || target.parts.length === 0) {
    throw new Error(`fixture target ${target.board_slug} has no firmware parts`);
  }
  for (const part of target.parts) {
    const artifactPath = safeJoin(immutableReleaseRoot, part.path);
    const artifact = await readFile(artifactPath);
    if (artifact.byteLength !== part.size || sha256(artifact) !== part.sha256) {
      throw new Error(`fixture artifact ${part.path} does not match its signed metadata`);
    }
  }
}

const stagedReleases = path.join(siteRoot, "releases");
await mkdir(siteRoot, { recursive: true });
await rm(stagedReleases, { recursive: true, force: true });
await cp(path.join(sourceRoot, "releases"), stagedReleases, { recursive: true });
await cp(path.join(sourceRoot, "minisign.pub"), path.join(stagedReleases, "minisign.pub"));
await safeFixtureFiles(stagedReleases);

const candidateHash = createHash("sha256");
for (const file of sourceFiles) {
  const relative = path.relative(sourceRoot, file).split(path.sep).join("/");
  candidateHash.update(`${relative}\0`);
  candidateHash.update(await readFile(file));
}
console.log(
  `staged signed browser fixture ${channel.version} (${candidateHash.digest("hex")})`,
);

async function requireSignature(documentPath) {
  const signaturePath = `${documentPath}.minisig`;
  const signature = await readFile(signaturePath, "utf8");
  if (
    !signature.includes("PRNS browser test fixture signature") ||
    !signature.includes("test fixture; no production trust")
  ) {
    throw new Error(`${signaturePath} is not the expected test-only signature`);
  }
}

function safeJoin(root, relative) {
  if (
    typeof relative !== "string" ||
    path.isAbsolute(relative) ||
    relative.includes("\\") ||
    relative.split("/").some((component) => !component || component === "." || component === "..")
  ) {
    throw new Error(`unsafe fixture artifact path ${JSON.stringify(relative)}`);
  }
  const joined = path.resolve(root, relative);
  if (!isWithin(root, joined)) {
    throw new Error(`fixture artifact escapes its release root: ${relative}`);
  }
  return joined;
}

function isWithin(root, candidate) {
  const relative = path.relative(root, candidate);
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}
