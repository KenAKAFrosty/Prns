import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { buildWasm } from "./build-wasm.mjs";

const packageRoot = fileURLToPath(new URL("../", import.meta.url));
const target = "wasm32-unknown-unknown";
const packageJson = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));

function runner(targetDirectory, failAt, failure = new Error("command failed")) {
  const calls = [];
  return {
    calls,
    run(command, args, options) {
      calls.push({ command, args, options });
      if (calls.length === failAt) throw failure;
      return JSON.stringify({ target_directory: targetDirectory });
    },
  };
}

for (const [script, profile, output] of [
  ["build:wasm", "debug", "smoke/pkg"],
  ["build:package:wasm", "release", "smoke/pkg"],
  ["build:playground:wasm", "release", "target/browser-playground/pkg"],
]) {
  test(`${script} preserves its profile and output directory`, () => {
    const prefix = "node scripts/build-wasm.mjs";
    const command = packageJson.scripts[script];
    assert.ok(command.startsWith(prefix));
    const args = command.slice(prefix.length).trim().split(/\s+/).filter(Boolean);
    const targetDirectory = join(packageRoot, "target");
    const fake = runner(targetDirectory);
    buildWasm(args, fake);
    assert.deepEqual(fake.calls.map(({ command, args }) => [command, args]), [
      ["cargo", ["metadata", "--locked", "--format-version", "1", "--no-deps"]],
      ["cargo", ["build", "--locked", "--target", target, ...(profile === "release" ? ["--release"] : [])]],
      ["wasm-bindgen", [join(targetDirectory, target, profile, "prns_wasm.wasm"), "--target", "web", "--out-dir", output]],
    ]);
    for (const call of fake.calls) {
      assert.equal(call.options.cwd, packageRoot);
      assert.equal(call.options.shell, undefined);
      assert.equal(call.options.env, undefined, "inherit Cargo configuration and remap environment");
    }
    assert.equal(fake.calls[0].options.encoding, "utf8");
    assert.deepEqual(fake.calls[0].options.stdio, ["inherit", "pipe", "inherit"]);
    assert.equal(fake.calls[1].options.stdio, "inherit");
    assert.equal(fake.calls[2].options.stdio, "inherit");
  });
}

test("Cargo metadata controls the input path while the requested output stays package-local", () => {
  const targetDirectory = resolve("external cargo cache with spaces");
  const fake = runner(targetDirectory);
  buildWasm(["--release", "--out-dir", "output with spaces/pkg"], fake);
  assert.deepEqual(fake.calls[2].args, [
    join(targetDirectory, target, "release", "prns_wasm.wasm"),
    "--target", "web", "--out-dir", "output with spaces/pkg",
  ]);
  assert.ok(fake.calls.every(call => call.options.cwd === packageRoot));
});

test("source-archive docs builds use the helper and forward their feature", () => {
  const staging = readFileSync(new URL("../../tools/build/stage-wasm-docs-browser-playground.sh", import.meta.url), "utf8");
  assert.match(staging, /node scripts\/build-wasm\.mjs --release --features source-archive \\\n\s+--out-dir target\/browser-playground\/pkg/);
  assert.doesNotMatch(staging, /wasm-bindgen|cargo build/);
  const fake = runner(resolve("cargo output"));
  buildWasm(["--release", "--features", "source-archive", "--out-dir", "target/browser-playground/pkg"], fake);
  assert.deepEqual(fake.calls[1].args, ["build", "--locked", "--target", target, "--release", "--features", "source-archive"]);
  assert.equal(fake.calls[2].args.at(-1), "target/browser-playground/pkg");
});

for (const [stage, failAt] of [["metadata", 1], ["build", 2], ["wasm-bindgen", 3]]) {
  test(`${stage} errors propagate without running later commands`, () => {
    const failure = Object.assign(new Error(`${stage} failed`), { status: 17 });
    const fake = runner(join(packageRoot, "target"), failAt, failure);
    assert.throws(() => buildWasm([], fake), error => error === failure);
    assert.equal(fake.calls.length, failAt);
  });
}

for (const directory of [undefined, null, "", "relative/target", 42]) {
  test(`invalid metadata target_directory ${JSON.stringify(directory)} fails before building`, () => {
    const fake = runner(directory);
    assert.throws(() => buildWasm([], fake), /absolute target_directory/);
    assert.equal(fake.calls.length, 1);
  });
}

test("malformed metadata fails before building", () => {
  let calls = 0;
  assert.throws(() => buildWasm([], { run() { calls += 1; return "invalid JSON"; } }), SyntaxError);
  assert.equal(calls, 1);
});

test("missing tool errors retain their original cause", () => {
  const failure = Object.assign(new Error("spawn cargo ENOENT"), { code: "ENOENT" });
  const fake = runner(join(packageRoot, "target"), 1, failure);
  assert.throws(() => buildWasm([], fake), error => error === failure);
  assert.equal(fake.calls.length, 1);
});

for (const args of [["--unknown"], ["--out-dir"], ["--out-dir", ""], ["--features", ""], ["unexpected"]]) {
  test(`invalid arguments ${JSON.stringify(args)} fail before running tools`, () => {
    const fake = runner(join(packageRoot, "target"));
    assert.throws(() => buildWasm(args, fake));
    assert.equal(fake.calls.length, 0);
  });
}
