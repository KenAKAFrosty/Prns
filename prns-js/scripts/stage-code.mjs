import { copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

await Promise.all([
  mkdir(resolve(packageRoot, "dist"), { recursive: true }),
  mkdir(resolve(packageRoot, "dist-cjs"), { recursive: true }),
]);
await copyFile(
  resolve(packageRoot, "scripts/commonjs-package.json"),
  resolve(packageRoot, "dist-cjs/package.json"),
);

const caseworkSource = await readFile(
  resolve(packageRoot, "node_modules/casework/briefcase.ts"),
  "utf8",
);
const compilerOptions = {
  target: ts.ScriptTarget.ES2022,
  removeComments: true,
};
const esm = ts.transpileModule(caseworkSource, {
  compilerOptions: {
    ...compilerOptions,
    module: ts.ModuleKind.ES2022,
  },
  fileName: "casework.ts",
});
const commonjs = ts.transpileModule(caseworkSource, {
  compilerOptions: {
    ...compilerOptions,
    module: ts.ModuleKind.CommonJS,
  },
  fileName: "casework.ts",
});
await Promise.all([
  writeFile(resolve(packageRoot, "dist/casework.js"), esm.outputText),
  writeFile(resolve(packageRoot, "dist-cjs/casework.js"), commonjs.outputText),
]);
