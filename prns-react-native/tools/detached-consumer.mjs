#!/usr/bin/env node
/** Install the actual npm archives outside the checkout, then check the consumer. */
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { fileURLToPath } from 'node:url';

export const runtimePatch = 'patches/@ubjs+react-native+0.31.0-6.patch';

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

export function validatePackagedAttribution(files, selection, record, notice) {
  for (const path of ['THIRD_PARTY_NOTICES.md', 'third-party-sources.json']) {
    assert(files.some(file => file.path === path), `missing packaged attribution: ${path}`);
  }
  assert(files.some(file => file.path === runtimePatch), 'missing required runtime patch');
  assert.equal(record.schemaVersion, 1, 'unsupported source record');
  assert.deepEqual(record.selection, selection, 'attribution belongs to another native image');
  assert.equal(createHash('sha256').update(notice).digest('hex'), record.notice_sha256, 'packaged notice digest mismatch');
  assert(record.runtime_archives?.packages?.length > 0, 'missing runtime archive provenance');
  consumerRuntimeDependencies(record.runtime_archives.packages);
  for (const runtime of record.runtime_archives.packages) {
    assert.match(runtime.sha256, /^[a-f0-9]{64}$/);
    assert(runtime.license_notice && runtime.upstream, 'missing runtime license/source record');
  }
}

export function consumerRuntimeDependencies(packages) {
  assert.equal(packages.length, 2, 'the consumer requires both recorded runtime packages');
  const dependencies = {};
  for (const runtime of packages) {
    assert(['@ubjs/core', '@ubjs/react-native'].includes(runtime.name), 'unexpected runtime package');
    assert(!Object.hasOwn(dependencies, runtime.name), 'duplicate runtime package');
    assert.match(runtime.version, /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/, 'runtime version must be exact');
    const name = runtime.name.split('/')[1];
    assert.equal(runtime.archive, `https://registry.npmjs.org/${runtime.name}/-/${name}-${runtime.version}.tgz`,
      'runtime archive must be its official registry URL');
    assert.match(runtime.integrity, /^sha512-[A-Za-z0-9+/]+={0,2}$/, 'missing runtime registry integrity');
    dependencies[runtime.name] = runtime.version;
  }
  return dependencies;
}

export function validateInstalledRuntimes(lock, packages) {
  consumerRuntimeDependencies(packages);
  for (const runtime of packages) {
    const installed = lock.packages?.[`node_modules/${runtime.name}`];
    assert(installed, `missing installed runtime ${runtime.name}`);
    for (const [field, expected] of [['version', runtime.version], ['resolved', runtime.archive], ['integrity', runtime.integrity]]) {
      assert.equal(installed[field], expected, `installed runtime ${runtime.name} ${field} differs from source record`);
    }
  }
}

export function consumerDevDependencies(sdkPackage) {
  // Keep the consumer's packed SDK/core and registry runtime selections intact.
  return Object.fromEntries(['typescript', '@types/react', 'patch-package'].map(name => {
    const version = sdkPackage.devDependencies[name];
    assert.match(version, /^\d+\.\d+\.\d+$/, `consumer tooling must have an exact version: ${name}`);
    return [name, version];
  }));
}

export function validateAppleSceneManifest(info) {
  const manifest = info.UIApplicationSceneManifest;
  assert(manifest, 'the built iOS example requires UIKit scene support');
  assert.equal(manifest.UIApplicationSupportsMultipleScenes, false);
  const configurations = manifest.UISceneConfigurations;
  const role = 'UIWindowSceneSessionRoleApplication';
  assert.deepEqual(Object.keys(configurations ?? {}), [role]);
  assert.equal(configurations[role].length, 1);
  // Expo's template supplies the app's SceneDelegate subclass; Xcode expands its module name.
  assert.match(configurations[role][0].UISceneDelegateClassName ?? '',
    /^[A-Za-z_][A-Za-z0-9_]*\.SceneDelegate$/, 'the built example must select its Expo SceneDelegate');
}

export function consumerOptions(args) {
  const { values } = parseArgs({ args, options: {
    android: { type: 'boolean', default: false },
    ios: { type: 'boolean', default: false },
    'require-ios': { type: 'boolean', default: false },
    'pack-only': { type: 'boolean', default: false },
    keep: { type: 'boolean', default: false },
    aggregate: { type: 'boolean', default: false },
    bindings: { type: 'string' },
    sdk: { type: 'string' },
  } });
  assert(!values.bindings || values.aggregate, '--bindings requires --aggregate');
  assert(!values['pack-only'] || (!values.android && !values.ios && !values.bindings), '--pack-only cannot compile Android/iOS or check composition bindings');
  if (values.ios) values['require-ios'] = true;
  return values;
}

export function main(args = process.argv.slice(2)) {
  const values = consumerOptions(args);
  assert(!values.ios || process.platform === 'darwin', '--ios requires macOS and Xcode');
  const source = resolve(dirname(fileURLToPath(import.meta.url)), '..');
  const sdk = values.sdk ? resolve(values.sdk) : source;
  const root = dirname(source);
  const selection = JSON.parse(readFileSync(join(sdk, 'native-image.json'), 'utf8'));
  validateSelection(selection, values.aggregate);
  const temporary = mkdtempSync(join(tmpdir(), 'prns-expo-consumer-'));
  const log = join(temporary, 'validation.log');
  const environment = { ...process.env };
  if (values.ios) {
    // Keep CocoaPods, Expo, npm and compiler writes inside the detached consumer.
    // RN's shared download cache has no location override; bypass it here.
    Object.assign(environment, {
      TMPDIR: join(temporary, 'tmp'),
      npm_config_cache: join(temporary, 'npm-cache'),
      CP_HOME_DIR: join(temporary, 'cocoapods'),
      CP_CACHE_DIR: join(temporary, 'cocoapods-cache'),
      CLANG_MODULE_CACHE_PATH: join(temporary, 'clang-module-cache'),
      __UNSAFE_EXPO_HOME_DIRECTORY: join(temporary, 'expo'),
      RCT_SKIP_CACHES: '1', COCOAPODS_DISABLE_STATS: 'true', EXPO_NO_TELEMETRY: '1', CI: '1',
    });
    mkdirSync(environment.TMPDIR, { recursive: true });
  }
  function run(command, commandArgs, cwd) {
    const result = spawnSync(command, commandArgs, { cwd, env: environment, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 });
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
    run('python3', [join(source, 'tools/package_notices.py'), '--destination', sdk, '--check-inputs'], root);
    const expectedSources = JSON.parse(readFileSync(join(sdk, 'third-party-sources.json'), 'utf8'));
    const sdkArchive = pack(sdk);
    const readMember = name => run('tar', ['-xOf', join(temporary, sdkArchive.filename), `package/${name}`], root);
    const sourceRecord = JSON.parse(readMember('third-party-sources.json'));
    validatePackagedAttribution(sdkArchive.files, selection, sourceRecord, readMember('THIRD_PARTY_NOTICES.md'));
    assert.deepEqual(sourceRecord, expectedSources, 'packed source record differs from verified inputs');
    const attribution = { sourceRecordSha256: digest(join(sdk, 'third-party-sources.json')),
      noticeSha256: sourceRecord.notice_sha256, policyApproval: false };

    const appleImages = validatePackagedAppleImages(sdkArchive.files, selection, values['require-ios']);
    const images = [...validatePackagedImages(sdkArchive.files, selection), ...appleImages];
    if (values['pack-only']) {
      writeFileSync(join(temporary, 'receipt.json'), `${JSON.stringify({
        schemaVersion: 1, selection, sdk: archiveReceipt(sdkArchive), attribution,
        androidImageContents: 'passed', iosFrameworkContents: appleImages.length ? 'passed' : 'not-run',
        strictTypeScript: 'not-run', androidSdkKotlin: 'not-run', iosSdkCompilation: 'not-run',
        iosSceneManifest: 'not-run', deviceRuntimeQualification: false,
      }, null, 2)}\n`);
      console.log(`Packed ${selection.provider} SDK contains the selected Android image${appleImages.length ? ' and iOS framework' : ''}; native compilation and runtime qualification were not run.`);
      if (values.keep) console.log(`Archive, receipt and log retained: ${temporary}`);
      else rmSync(temporary, { recursive: true, force: true });
      return;
    }
    const coreArchive = pack(join(root, 'prns-js'));
    const manifest = JSON.parse(readFileSync(join(source, 'example/package.json'), 'utf8'));
    manifest.dependencies['personal-rns-expo'] = `file:./${sdkArchive.filename}`;
    manifest.dependencies['personal-rns'] = `file:./${coreArchive.filename}`;
    Object.assign(manifest.dependencies, consumerRuntimeDependencies(sourceRecord.runtime_archives.packages));
    let bindingsArchive;
    const includes = ['App.tsx', 'index.ts'];
    if (values.bindings) {
      const directory = resolve(values.bindings);
      const bindings = JSON.parse(readFileSync(join(directory, 'package.json'), 'utf8'));
      bindingsArchive = pack(directory);
      assert(!bindingsArchive.files.some(file => file.path.endsWith('.so') || file.path.includes('.xcframework/')), 'composition bindings must borrow the SDK image');
      manifest.dependencies[bindings.name] = `file:./${bindingsArchive.filename}`;
      // Private compositions may select workspace-relative packages. Resolve those
      // dependencies to the same selected inputs without rewriting either archive.
      manifest.overrides = { [bindings.name]: Object.fromEntries(
        ['personal-rns-expo', 'personal-rns', '@ubjs/core', '@ubjs/react-native'].map(name => [name, `$${name}`]),
      ) };
      const entry = bindings.exports?.['./native'] ? `${bindings.name}/native` : bindings.name;
      writeFileSync(join(temporary, 'composition-probe.ts'), `import * as composition from ${JSON.stringify(entry)};\nvoid composition;\n`);
      includes.push('composition-probe.ts');
    }
    manifest.devDependencies = consumerDevDependencies(JSON.parse(readFileSync(join(source, 'package.json'), 'utf8')));
    writeFileSync(join(temporary, 'package.json'), `${JSON.stringify(manifest, null, 2)}\n`);
    for (const file of ['App.tsx', 'index.ts', 'app.json']) cpSync(join(source, 'example', file), join(temporary, file));
    writeFileSync(join(temporary, 'tsconfig.json'), JSON.stringify({ compilerOptions: {
      target: 'ES2022', module: 'ESNext', moduleResolution: 'Bundler', lib: ['ES2022', 'DOM'],
      strict: true, noEmit: true, exactOptionalPropertyTypes: true, noUncheckedIndexedAccess: true,
      skipLibCheck: true, jsx: 'react-jsx', allowImportingTsExtensions: true,
    }, include: includes }, null, 2));
    console.log(`Installing packed ${values.aggregate ? 'aggregate' : 'default'} SDK, core contract, and registry runtime in a detached consumer.`);
    run('npm', ['install', '--ignore-scripts', '--no-audit', '--no-fund'], temporary);
    validateInstalledRuntimes(JSON.parse(readFileSync(join(temporary, 'package-lock.json'), 'utf8')),
      sourceRecord.runtime_archives.packages);
    run('npm', ['run', 'patch:runtime'], temporary);
    run('npx', ['--no-install', 'tsc', '-p', 'tsconfig.json'], temporary);
    let androidUnitTests = null;
    if (values.android) {
      run('npx', ['--no-install', 'expo', 'prebuild', '--platform', 'android', '--no-install'], temporary);
      // Test the packed production module with its maintained source tests; the
      // tests stay outside the published archive and add no runtime dependency.
      environment.PRNS_SDK_TEST_SOURCES = join(source, 'android/src/test/java');
      const init = join(temporary, 'sdk-tests.gradle');
      writeFileSync(init, `allprojects { project ->
  if (project.name == 'personal-rns-expo') {
    project.plugins.withId('com.android.library') {
      project.afterEvaluate {
        project.extensions.getByName('android').sourceSets.getByName('test').java.srcDir(
          new File(System.getenv('PRNS_SDK_TEST_SOURCES')))
      }
    }
  }
}
`);
      run('./gradlew', [':personal-rns-expo:compileDebugKotlin', ':personal-rns-expo:testDebugUnitTest',
        '--init-script', init, '--max-workers=4', '--console=plain', '-PreactNativeArchitectures=arm64-v8a'], join(temporary, 'android'));
      const reports = join(temporary, 'node_modules/personal-rns-expo/android/build/test-results/testDebugUnitTest');
      androidUnitTests = JSON.parse(run('python3', ['-c', `import json, pathlib, sys, xml.etree.ElementTree as ET
files = sorted(pathlib.Path(sys.argv[1]).glob('TEST-*.xml'))
if not files: raise SystemExit('SDK unit test reports are missing')
counts = {name: sum(int(ET.parse(path).getroot().get(name, '0')) for path in files)
          for name in ('tests', 'failures', 'errors', 'skipped')}
if not counts['tests'] or counts['failures'] or counts['errors']: raise SystemExit(str(counts))
print(json.dumps(counts))`, reports], temporary));
    }
    if (values.ios) {
      run('npx', ['--no-install', 'expo', 'prebuild', '--platform', 'ios', '--no-install'], temporary);
      const ios = join(temporary, 'ios');
      run('pod', ['install'], ios);
      const workspaces = readdirSync(ios).filter(name => name.endsWith('.xcworkspace'));
      assert.equal(workspaces.length, 1, 'expected one detached iOS example workspace');
      const workspace = workspaces[0];
      const scheme = workspace.slice(0, -'.xcworkspace'.length);
      run('xcrun', ['xcodebuild', '-workspace', join(ios, workspace), '-scheme', scheme,
        '-configuration', 'Debug', '-sdk', 'iphonesimulator', '-destination', 'generic/platform=iOS Simulator',
        '-derivedDataPath', join(temporary, 'DerivedData'),
        '-clonedSourcePackagesDirPath', join(temporary, 'SourcePackages'),
        '-resultBundlePath', join(temporary, 'ios-build.xcresult'), '-jobs', '4', '-quiet',
        'ARCHS=arm64', 'ONLY_ACTIVE_ARCH=YES',
        'CODE_SIGNING_ALLOWED=NO', 'CODE_SIGNING_REQUIRED=NO', 'build'], ios);
      const info = join(temporary, 'DerivedData/Build/Products/Debug-iphonesimulator', `${scheme}.app/Info.plist`);
      validateAppleSceneManifest(JSON.parse(run('plutil', ['-convert', 'json', '-o', '-', info], ios)));
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
      schemaVersion: 1, selection, sdk: archiveReceipt(sdkArchive), attribution, core: archiveReceipt(coreArchive),
      bindings: bindingsArchive ? archiveReceipt(bindingsArchive) : null, images: imageReceipts,
      registryRuntimeIntegrity: 'passed', runtimePatch: 'applied',
      strictTypeScript: 'passed', androidSdkKotlin: values.android ? 'passed' : 'not-run', androidUnitTests,
      iosSdkCompilation: values.ios ? 'passed' : 'not-run',
      iosSceneManifest: values.ios ? 'passed' : 'not-run',
      iosFrameworkContents: appleImages.length ? 'passed' : 'not-run',
      deviceRuntimeQualification: false,
    }, null, 2)}\n`);
    console.log(`Detached packed ${values.aggregate ? 'aggregate' : 'default'} consumer passed strict TypeScript${values.android ? ' and Android SDK compilation' : ''}${values.ios ? ' and iOS simulator SDK compilation' : ''}${bindingsArchive ? ' including composition bindings' : ''}.`);
    if (values.keep) console.log(`Consumer, receipt and log retained: ${temporary}`);
    else rmSync(temporary, { recursive: true, force: true });
  } catch (error) {
    console.error(`Detached consumer retained for diagnosis: ${temporary}`);
    throw error;
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
