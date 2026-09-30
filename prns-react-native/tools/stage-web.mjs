#!/usr/bin/env node
// Publish the existing browser engine assets; protocol execution stays in personal-rns.
import { cpSync, mkdirSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
const example = resolve(dirname(fileURLToPath(import.meta.url)), '../example');
const require = createRequire(resolve(example, 'package.json'));
const core = dirname(require.resolve('personal-rns/package.json'));
const destination = resolve(example, 'public/prns');
mkdirSync(destination, { recursive: true });
for (const name of ['prns_wasm.js', 'prns_wasm_bg.wasm']) cpSync(resolve(core, 'wasm', name), resolve(destination, name));
console.log(`Existing PRNS browser engine assets staged at ${destination}`);
