import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

assert.equal(
  process.platform,
  "darwin",
  "ios:test requires macOS with Xcode; it cannot run on this host",
);
const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

const recoveryTestDirectory = mkdtempSync(resolve(tmpdir(), "prns-protected-data-recovery-"));
try {
  const recoveryTestExecutable = resolve(recoveryTestDirectory, "recovery-tests");
  execFileSync(
    "xcrun",
    [
      "swiftc",
      resolve(packageRoot, "ios/PrnsProtectedDataRecovery.swift"),
      resolve(packageRoot, "ios/PrnsRestorationDispatch.swift"),
      resolve(packageRoot, "scripts/PrnsProtectedDataRecoveryTests.swift"),
      "-o",
      recoveryTestExecutable,
    ],
    { stdio: "inherit" },
  );
  execFileSync(recoveryTestExecutable, [], { stdio: "inherit" });

  const dispatchTestExecutable = resolve(recoveryTestDirectory, "start-dispatch-tests");
  execFileSync(
    "xcrun",
    [
      "swiftc",
      "-warnings-as-errors",
      resolve(packageRoot, "ios/PrnsNativeStartDispatch.swift"),
      resolve(packageRoot, "scripts/PrnsNativeStartDispatchTests.swift"),
      "-o",
      dispatchTestExecutable,
    ],
    { stdio: "inherit" },
  );
  execFileSync(dispatchTestExecutable, [], { stdio: "inherit", timeout: 15000 });

  const probeSource = resolve(packageRoot, "ios/PrnsAppRestorationProbe.swift");
  const probeTestExecutable = resolve(recoveryTestDirectory, "probe-tests");
  execFileSync(
    "xcrun",
    [
      "swiftc",
      "-D",
      "DEBUG",
      resolve(packageRoot, "ios/PrnsIosDiagnostics.swift"),
      probeSource,
      resolve(packageRoot, "scripts/PrnsAppRestorationProbeTests.swift"),
      "-o",
      probeTestExecutable,
    ],
    { stdio: "inherit" },
  );
  const probeResult = spawnSync(probeTestExecutable, [], { encoding: "utf8" });
  assert.ifError(probeResult.error);
  assert.equal(probeResult.status, 0, probeResult.stderr);
  assert.equal(probeResult.stdout, "");
  const probeTag = "PRNS_IOS_";
  const probeLines = probeResult.stderr
    .split("\n")
    .filter((line) => line.includes(probeTag))
    .map((line) => line.slice(line.indexOf(probeTag)));
  assert.deepEqual(
    probeLines,
    [
      "PRNS_IOS_LIFECYCLE launch centralRestoration=true protectedData=false",
      "PRNS_IOS_ASK phase=ready picker=idle authorized=1 nativeStart=running restoration=true",
      "PRNS_IOS_LIFECYCLE prepare outcome=prepared stage=none",
      "PRNS_IOS_LIFECYCLE start outcome=failed stage=runtime",
      "PRNS_IOS_LIFECYCLE prepare outcome=unknown stage=unknown",
      "PRNS_IOS_RESTORATION sequence=17 event=logger_installed",
      "PRNS_IOS_RESTORATION sequence=18 event=central_scan_already_scanning",
      "PRNS_IOS_RESTORATION sequence=19 event=gatt_control_hello_sent",
      "PRNS_IOS_RESTORATION sequence=20 event=gatt_control_welcome_received",
      "PRNS_IOS_RESTORATION sequence=21 event=central_closed_session_reaped",
      "PRNS_IOS_RESTORATION sequence=18446744073709551615 event=central_scan_started",
    ],
    "each diagnostic channel must reach stderr once; invalid probe codes must stay silent",
  );
  assert.doesNotMatch(
    probeResult.stderr,
    /private-peer|private-error|private-outcome|private-stage|gatt_control_timeout/,
    "unknown native values and rejected restoration payloads must never become public",
  );
  const releaseProbeObject = resolve(recoveryTestDirectory, "probe-release.o");
  execFileSync(
    "xcrun",
    ["swiftc", "-parse-as-library", "-emit-object", probeSource, "-o", releaseProbeObject],
    { stdio: "inherit" },
  );
  const releaseProbeSymbols = execFileSync("xcrun", ["nm", "-g", releaseProbeObject], {
    encoding: "utf8",
  });
  assert.doesNotMatch(
    releaseProbeSymbols,
    /prns_app_ios_restoration_probe_emit|prnsAppIosRestorationProbeEmit/,
    "non-Debug compilation must omit the diagnostic callback entirely",
  );
  const releaseDiagnosticsObject = resolve(recoveryTestDirectory, "diagnostics-release.o");
  execFileSync(
    "xcrun",
    [
      "swiftc",
      "-parse-as-library",
      "-emit-object",
      resolve(packageRoot, "ios/PrnsIosDiagnostics.swift"),
      "-o",
      releaseDiagnosticsObject,
    ],
    { stdio: "inherit" },
  );
  const releaseDiagnosticsSymbols = execFileSync("xcrun", ["nm", "-g", releaseDiagnosticsObject], {
    encoding: "utf8",
  });
  assert.doesNotMatch(
    releaseDiagnosticsSymbols,
    /\b_fputs\b/,
    "non-Debug diagnostics must omit the stderr mirror",
  );
} finally {
  rmSync(recoveryTestDirectory, { force: true, recursive: true });
}

console.log(
  "ios:test: Swift startup dispatch, recovery, restoration diagnostics and release-symbol checks passed",
);
