import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm, symlink, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { safeFixtureFiles } from "../browser/support/fixture-safety.mjs";

async function fixture(t) {
  const root = await mkdtemp(path.join(os.tmpdir(), "prns-browser-fixture-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  return root;
}

test("fixture safety accepts public release files and signatures", async (t) => {
  const root = await fixture(t);
  await mkdir(path.join(root, "releases", "0.2.6"), { recursive: true });
  await writeFile(path.join(root, "minisign.pub"), "untrusted comment: minisign public key 0000000000000000\n");
  await writeFile(path.join(root, "releases", "0.2.6", "manifest.json.minisig"), "public signature\n");

  const files = await safeFixtureFiles(root);
  assert.equal(files.length, 2);
});

test("fixture safety rejects a symbolic link at any depth", async (t) => {
  const root = await fixture(t);
  const nested = path.join(root, "releases", "0.2.6");
  await mkdir(nested, { recursive: true });
  const outside = path.join(root, "outside.bin");
  await writeFile(outside, "outside");
  await symlink(outside, path.join(nested, "linked.bin"));

  await assert.rejects(safeFixtureFiles(root), /symbolic link.*linked\.bin/i);
});

test("fixture safety rejects private-key filenames", async (t) => {
  const root = await fixture(t);
  await mkdir(path.join(root, "nested"));
  await writeFile(path.join(root, "nested", "release-signing-key.pem"), "opaque bytes");

  await assert.rejects(safeFixtureFiles(root), /private-key material by name/i);
});

test("fixture safety rejects private-key content under an innocent name", async (t) => {
  const root = await fixture(t);
  await writeFile(
    path.join(root, "candidate.txt"),
    "untrusted comment: minisign encrypted secret key\nRWQAAATESTONLY\n",
  );

  await assert.rejects(safeFixtureFiles(root), /private-key material: candidate\.txt/i);
});
