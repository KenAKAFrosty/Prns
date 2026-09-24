import assert from 'node:assert/strict';
import test from 'node:test';
import { loadWasmModule } from '../dist/browser/bootstrap.js';

const moduleUrl = (source) => new URL(`data:text/javascript,${encodeURIComponent(source)}`);

test('browser WASM loader imports the requested URL and initializes its module', async () => {
  const result = await loadWasmModule(moduleUrl('export let initialized = false; export default async () => { initialized = true; };'));
  assert.equal(result.tag, 'Loaded');
  assert.equal(result.data.initialized, true);
});

test('browser WASM loader preserves typed missing-initializer failure', async () => {
  const result = await loadWasmModule(moduleUrl('export const marker = 1;'));
  assert.equal(result.tag, 'WasmLoadFailed');
  assert.match(result.data.detail, /no initializer/);
});

test('browser WASM loader preserves typed initializer failure', async () => {
  const result = await loadWasmModule(moduleUrl('export default async () => { throw new Error("fixture failed"); };'));
  assert.equal(result.tag, 'WasmLoadFailed');
  assert.match(result.data.detail, /fixture failed/);
});
