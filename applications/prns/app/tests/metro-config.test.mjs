import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { realpathSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const require = createRequire(import.meta.url);
const appRoot = fileURLToPath(new URL('../', import.meta.url));
const { getDefaultConfig } = require('expo/metro-config');
const config = require('../metro.config.cjs');

test('Metro watches the external linked JavaScript package without broadening to the repository', () => {
  const defaults = getDefaultConfig(appRoot);
  const packageRoot = path.dirname(realpathSync(require.resolve('personal-rns/package.json')));
  assert.deepEqual(config.watchFolders, [...new Set([...defaults.watchFolders, packageRoot])]);
  assert.ok(!config.watchFolders.includes(path.resolve(appRoot, '../../..')));
  assert.deepEqual(config.resolver, defaults.resolver);
});
