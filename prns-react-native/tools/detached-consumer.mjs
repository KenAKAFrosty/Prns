#!/usr/bin/env node
/** Install the actual npm archives outside the checkout, then check the consumer. */
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { cpSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { fileURLToPath } from 'node:url';

export function validateSelection(selection, aggregate) {
  assert.equal(selection.schemaVersion, 1, 'unsupported native image selection');
  assert.match(selection.libraryName, /^[a-z][a-z0-9_]*$/, 'invalid selected library name');
  assert.match(selection.provider, /^[a-z][a-z0-9_-]*$/, 'invalid selected provider');
  if (aggregate) assert.notEqual(selection.provider, 'default', '--aggregate requires an aggregate image selection');
  else assert.equal(selection.provider, 'default', 'the default consumer gate requires the default SDK image; use --aggregate for a composition');
}

export function validatePackagedImages(files, selection) {
  const images = files.map(file => file.path).filter(path => path.endsWith('.so'));
  const abis = new Set();
  for (const path of images) {
    const match = /^android\/src\/main\/jniLibs\/([^/]+)\/([^/]+)$/.exec(path);
    assert(match && match[2] === `lib${selection.libraryName}.so`, `unexpected native image ${path}`);
    assert(!abis.has(match[1]), `multiple selected images for ABI ${match[1]}`);
    abis.add(match[1]);
  }
  assert(abis.has('arm64-v8a'), 'the selected Android arm64 image must be shipped');
  return images;
}

export function validatePackagedAppleImages(files, selection, required = false) {
  const images = files.map(file => file.path).filter(path => path.includes('.xcframework/'));
  if (!required && images.length === 0) return images;
  const root = `ios/${selection.libraryName}.xcframework`;
  assert(images.includes(`${root}/Info.plist`), `the selected iOS framework must be shipped: ${root}`);
  const frameworks = new Set();
  for (const path of images) {
    assert(path.startsWith(`${root}/`), `unexpected native framework ${path}`);
    const match = /^(.*\/([^/]+)\.framework)\//.exec(path);
    if (match) {
      assert.equal(match[2], selection.libraryName, `unexpected native framework ${path}`);
      frameworks.add(match[1]);
    }
  }
  assert(frameworks.size > 0, `the selected iOS framework has no library slices: ${root}`);
  for (const framework of frameworks) {
    assert(images.includes(`${framework}/Info.plist`), `missing framework metadata: ${framework}`);
    assert(images.includes(`${framework}/${selection.libraryName}`), `missing framework binary: ${framework}`);
  }
  return images;
}

export function consumerDevDependencies(sdkPackage) {
  // Source SDK development selects sibling packages and pinned vendor archives.
  // Detached consumers must keep the packed runtime/core selections above instead.
  return Object.fromEntries(['typescript', '@types/react'].map(name => {
    const version = sdkPackage.devDependencies[name];
    assert.match(version, /^\d+\.\d+\.\d+$/, `consumer tooling must have an exact version: ${name}`);
    return [name, version];
  }));
}

export function main(args = process.argv.slice(2)) {
  const { values } = parseArgs({ args, options: {
    android: { type: 'boolean', default: false },
    'require-ios': { type: 'boolean', default: false },
    'pack-only': { type: 'boolean', default: false },
    keep: { type: 'boolean', default: false },
    aggregate: { type: 'boolean', default: false },
    bindings: { type: 'string' },
  } });
  assert(!values.bindings || values.aggregate, '--bindings requires --aggregate');
  assert(!values['pack-only'] || (!values.android && !values.bindings), '--pack-only cannot compile Android or check composition bindings');
  const sdk = resolve(dirname(fileURLToPath(import.meta.url)), '..');
  const root = dirname(sdk);
  const selection = JSON.parse(readFileSync(join(sdk, 'native-image.json'), 'utf8'));
  validateSelection(selection, values.aggregate);
  const temporary = mkdtempSync(join(tmpdir(), 'prns-expo-consumer-'));
  const log = join(temporary, 'validation.log');
  function run(command, commandArgs, cwd) {
    const result = spawnSync(command, commandArgs, { cwd, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 });
    writeFileSync(log, `${command} ${commandArgs.join(' ')}\n${result.stdout ?? ''}${result.stderr ?? ''}\n`, { flag: 'a' });
    if (result.error) throw result.error;
    assert.equal(result.status, 0, `${command} failed; see ${log}\n${result.stderr ?? ''}`);
    return result.stdout;
  }
  function pack(directory) {
    const [result] = JSON.parse(run('npm', ['pack', '--ignore-scripts', '--json', '--pack-destination', temporary], directory));
    for (const file of result.files) {
      assert(!/(^|\/)(build|\.gradle|node_modules|applications)(\/|$)/.test(file.path), `unexpected packaged file ${file.path}`);
    }
    return result;
  }
  const digest = path => createHash('sha256').update(readFileSync(path)).digest('hex');
  const archiveReceipt = archive => ({ name: archive.name, version: archive.version,
    filename: archive.filename, sha256: digest(join(temporary, archive.filename)), integrity: archive.integrity });
  try {
    const sdkArchive = pack(sdk);
    const appleImages = validatePackagedAppleImages(sdkArchive.files, selection, values['require-ios']);
    const images = [...validatePackagedImages(sdkArchive.files, selection), ...appleImages];
    if (values['pack-only']) {
      writeFileSync(join(temporary, 'receipt.json'), `${JSON.stringify({
        schemaVersion: 1, selection, sdk: archiveReceipt(sdkArchive),
        androidImageContents: 'passed', iosFrameworkContents: appleImages.length ? 'passed' : 'not-run',
        strictTypeScript: 'not-run', androidSdkKotlin: 'not-run', deviceRuntimeQualification: false,
      }, null, 2)}\n`);
      console.log(`Packed ${selection.provider} SDK contains the selected Android image${appleImages.length ? ' and iOS framework' : ''}; native compilation and runtime qualification were not run.`);
      if (values.keep) console.log(`Archive, receipt and log retained: ${temporary}`);
      else rmSync(temporary, { recursive: true, force: true });
      return;
    }
    const coreArchive = pack(join(root, 'prns-js'));
    const manifest = JSON.parse(readFileSync(join(sdk, 'example/package.json'), 'utf8'));
    manifest.dependencies['personal-rns-expo'] = `file:./${sdkArchive.filename}`;
    manifest.dependencies['personal-rns'] = `file:./${coreArchive.filename}`;
    for (const name of ['core', 'react-native']) {
      const archive = `ubjs-${name}-0.31.0-5.tgz`;
      cpSync(join(root, 'vendor/ubrn/packages', archive), join(temporary, archive));
      manifest.dependencies[`@ubjs/${name}`] = `file:./${archive}`;
    }
    let bindingsArchive;
    const includes = ['App.tsx', 'index.ts'];
    if (values.bindings) {
      const directory = resolve(values.bindings);
      const bindings = JSON.parse(readFileSync(join(directory, 'package.json'), 'utf8'));
      bindingsArchive = pack(directory);
      assert(!bindingsArchive.files.some(file => file.path.endsWith('.so') || file.path.includes('.xcframework/')), 'composition bindings must borrow the SDK image');
      manifest.dependencies[bindings.name] = `file:./${bindingsArchive.filename}`;
      // Private compositions may select workspace-relative packages. Resolve those
      // dependencies to the same packed inputs without rewriting either archive.
      manifest.overrides = { [bindings.name]: Object.fromEntries(
        ['personal-rns-expo', 'personal-rns', '@ubjs/core', '@ubjs/react-native'].map(name => [name, `$${name}`]),
      ) };
      const entry = bindings.exports?.['./native'] ? `${bindings.name}/native` : bindings.name;
      writeFileSync(join(temporary, 'composition-probe.ts'), `import * as composition from ${JSON.stringify(entry)};\nvoid composition;\n`);
      includes.push('composition-probe.ts');
    }
    manifest.devDependencies = consumerDevDependencies(JSON.parse(readFileSync(join(sdk, 'package.json'), 'utf8')));
    writeFileSync(join(temporary, 'package.json'), `${JSON.stringify(manifest, null, 2)}\n`);
    for (const file of ['App.tsx', 'index.ts', 'app.json']) cpSync(join(sdk, 'example', file), join(temporary, file));
    writeFileSync(join(temporary, 'tsconfig.json'), JSON.stringify({ compilerOptions: {
      target: 'ES2022', module: 'ESNext', moduleResolution: 'Bundler', lib: ['ES2022', 'DOM'],
      strict: true, noEmit: true, exactOptionalPropertyTypes: true, noUncheckedIndexedAccess: true,
      skipLibCheck: true, jsx: 'react-jsx', allowImportingTsExtensions: true,
    }, include: includes }, null, 2));
    console.log(`Installing packed ${values.aggregate ? 'aggregate' : 'default'} SDK, core contract, and pinned runtime in a detached consumer.`);
    run('npm', ['install', '--ignore-scripts', '--no-audit', '--no-fund'], temporary);
    run('npx', ['--no-install', 'tsc', '-p', 'tsconfig.json'], temporary);
    if (values.android) {
      run('npx', ['--no-install', 'expo', 'prebuild', '--platform', 'android', '--no-install'], temporary);
      run('./gradlew', [':personal-rns-expo:compileDebugKotlin', '--max-workers=4', '--console=plain', '-PreactNativeArchitectures=arm64-v8a'], join(temporary, 'android'));
    }
    const installedSdk = join(temporary, 'node_modules/personal-rns-expo');
    assert.deepEqual(JSON.parse(readFileSync(join(installedSdk, 'native-image.json'), 'utf8')), selection);
    const imageReceipts = images.map(path => {
      const sha256 = digest(join(sdk, path));
      assert.equal(digest(join(installedSdk, path)), sha256, `installed image differs: ${path}`);
      return { path, sha256 };
    });
    if (bindingsArchive) assert(!existsSync(join(temporary, 'node_modules', bindingsArchive.name, 'node_modules/personal-rns-expo')), 'composition must share the installed SDK');
    writeFileSync(join(temporary, 'receipt.json'), `${JSON.stringify({
      schemaVersion: 1, selection, sdk: archiveReceipt(sdkArchive), core: archiveReceipt(coreArchive),
      bindings: bindingsArchive ? archiveReceipt(bindingsArchive) : null, images: imageReceipts,
      strictTypeScript: 'passed', androidSdkKotlin: values.android ? 'passed' : 'not-run',
      iosFrameworkContents: appleImages.length ? 'passed' : 'not-run',
      deviceRuntimeQualification: false,
    }, null, 2)}\n`);
    console.log(`Detached packed ${values.aggregate ? 'aggregate' : 'default'} consumer passed strict TypeScript${values.android ? ' and Android SDK compilation' : ''}${bindingsArchive ? ' including composition bindings' : ''}.`);
    if (values.keep) console.log(`Consumer, receipt and log retained: ${temporary}`);
    else rmSync(temporary, { recursive: true, force: true });
  } catch (error) {
    console.error(`Detached consumer retained for diagnosis: ${temporary}`);
    throw error;
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
