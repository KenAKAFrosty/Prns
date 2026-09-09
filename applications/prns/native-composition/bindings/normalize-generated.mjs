// Deterministic cleanup for ubrn's --no-format output; no semantic rewrites.
import { readFileSync, writeFileSync } from 'node:fs';
for (const name of ['prns_app.ts', 'prns_app-ffi.ts']) {
  const path = new URL(`./typescript/${name}`, import.meta.url);
  writeFileSync(path, `${readFileSync(path, 'utf8').replace(/[ \t]+$/gm, '').trimEnd()}\n`);
}
