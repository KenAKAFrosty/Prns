import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
assert.equal(process.platform, 'darwin', 'Apple helper tests require macOS');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const temporary = mkdtempSync(resolve(tmpdir(), 'prns-platform-tests-'));
try {
  for (const [name, inputs] of [
    ['PrnsProtectedDataRecovery', ['PrnsProtectedDataRecovery', 'PrnsRestorationDispatch']],
    ['PrnsBluetoothAuthorization', ['PrnsBluetoothAuthorization', 'PrnsRestorationDispatch']],
    ['PrnsNativeStartDispatch', ['PrnsNativeStartDispatch']],
    ['PrnsBluetoothRuntimeCoordinator', ['PrnsBluetoothAuthorization', 'PrnsRestorationDispatch', 'PrnsBluetoothRuntimeCoordinator']],
  ]) {
    const executable = resolve(temporary, name);
    execFileSync('xcrun', ['swiftc', '-warnings-as-errors', ...inputs.map(input => resolve(root, `ios/${input}.swift`)),
      resolve(root, `tests/apple/${name}Tests.swift`), '-o', executable], { stdio: 'inherit' });
    execFileSync(executable, [], { stdio: 'inherit', timeout: 15000 });
  }
} finally { rmSync(temporary, { recursive: true, force: true }); }
