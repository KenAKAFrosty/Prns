import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import test from 'node:test';
import { consumerDevDependencies, consumerRuntimeDependencies, consumerOptions, runtimePatch, validateInstalledRuntimes, validatePackagedAttribution, validateAppleSceneManifest, validatePackagedAppleImages, validatePackagedImages, validateSelection } from '../tools/detached-consumer.mjs';

const defaultImage = { schemaVersion: 1, provider: 'default', libraryName: 'prns_host_mobile' };
const aggregateImage = { schemaVersion: 1, provider: 'sample-composition', libraryName: 'sample_composition' };
const file = (abi, image) => ({ path: `android/src/main/jniLibs/${abi}/lib${image}.so` });
const frameworkFiles = image => [
  `ios/${image}.xcframework/Info.plist`,
  `ios/${image}.xcframework/ios-arm64-simulator/${image}.framework/Info.plist`,
  `ios/${image}.xcframework/ios-arm64-simulator/${image}.framework/${image}`,
].map(path => ({ path }));

test('iOS compilation requires a packaged framework and cannot be reduced to an archive check', () => {
  const ios = consumerOptions(['--ios']);
  assert.equal(ios['require-ios'], true);
  assert.equal(ios.android, false);
  assert.throws(() => consumerOptions(['--ios', '--pack-only']), /cannot compile/);
  assert.throws(() => consumerOptions(['--android', '--pack-only']), /cannot compile/);
  assert.equal(consumerOptions(['--require-ios', '--pack-only'])['pack-only'], true);
});

test('the built iOS example must use Expo scene lifecycle configuration', () => {
  const configuration = { UISceneDelegateClassName: 'PRNSHost.SceneDelegate' };
  const manifest = { UIApplicationSupportsMultipleScenes: false,
    UISceneConfigurations: { UIWindowSceneSessionRoleApplication: [configuration] } };
  validateAppleSceneManifest({ UIApplicationSceneManifest: manifest });
  assert.throws(() => validateAppleSceneManifest({}), /requires UIKit scene support/);
  assert.throws(() => validateAppleSceneManifest({ UIApplicationSceneManifest: {
    ...manifest, UISceneConfigurations: {},
  } }));
  for (const invalid of ['UnrelatedSceneDelegate', 'EXExpoAppSceneDelegate',
    '$(PRODUCT_MODULE_NAME).SceneDelegate', 'PRNSHost.UnrelatedSceneDelegate', undefined]) {
    configuration.UISceneDelegateClassName = invalid;
    assert.throws(() => validateAppleSceneManifest({ UIApplicationSceneManifest: manifest }),
      /must select its Expo SceneDelegate/);
  }
});

test('detached consumer tooling cannot override packed dependencies with source paths', () => {
  const sourcePackage = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));
  const dependencies = consumerDevDependencies(sourcePackage);
  assert.deepEqual(Object.keys(dependencies).sort(), ['@types/react', 'patch-package', 'typescript']);
  assert(Object.values(dependencies).every(value => /^\d+\.\d+\.\d+$/.test(value)));
  assert.throws(() => consumerDevDependencies({ devDependencies: {
    ...sourcePackage.devDependencies, typescript: 'file:../tooling',
  } }), /exact version/);
});

test('image provider mode must be selected explicitly', () => {
  validateSelection(defaultImage, false);
  validateSelection(aggregateImage, true);
  assert.throws(() => validateSelection(aggregateImage, false), /use --aggregate/);
  assert.throws(() => validateSelection(defaultImage, true), /requires an aggregate/);
});

test('rejects malformed image metadata before packing', () => {
  assert.throws(() => validateSelection({ ...aggregateImage, schemaVersion: 2 }, true));
  assert.throws(() => validateSelection({ ...aggregateImage, libraryName: '../image' }, true));
  assert.throws(() => validateSelection({ ...aggregateImage, provider: '' }, true));
});

test('one selected image per ABI is accepted', () => {
  const files = [file('arm64-v8a', 'sample_composition'), file('x86_64', 'sample_composition')];
  assert.equal(validatePackagedImages(files, aggregateImage).length, 2);
});

test('rejects default and aggregate images packed together', () => {
  assert.throws(() => validatePackagedImages([
    file('arm64-v8a', 'sample_composition'), file('arm64-v8a', 'prns_host_mobile'),
  ], aggregateImage), /unexpected native image/);
  assert.throws(() => validatePackagedImages([
    file('arm64-v8a', 'sample_composition'), file('arm64-v8a', 'sample_composition'),
  ], aggregateImage), /multiple selected images/);
});

test('rejects missing arm64 images and binaries outside the SDK owner', () => {
  assert.throws(() => validatePackagedImages([file('x86_64', 'sample_composition')], aggregateImage), /arm64/);
  assert.throws(() => validatePackagedImages([{ path: 'extra/libsample_composition.so' }], aggregateImage), /unexpected native image/);
});

test('iOS contents are optional for Android checks and required for distribution checks', () => {
  const androidOnly = [file('arm64-v8a', aggregateImage.libraryName)];
  assert.deepEqual(validatePackagedAppleImages(androidOnly, aggregateImage), []);
  assert.throws(() => validatePackagedAppleImages(androidOnly, aggregateImage, true), /must be shipped/);
  const complete = frameworkFiles(aggregateImage.libraryName);
  assert.deepEqual(validatePackagedAppleImages(complete, aggregateImage, true), complete.map(file => file.path));
  assert.throws(() => validatePackagedAppleImages(complete.slice(0, 1), aggregateImage), /no library slices/);
  assert.throws(() => validatePackagedAppleImages(complete.slice(0, -1), aggregateImage), /missing framework binary/);
  assert.throws(() => validatePackagedAppleImages([complete[0], complete[2]], aggregateImage), /missing framework metadata/);
  assert.throws(() => validatePackagedAppleImages([...complete, ...frameworkFiles(defaultImage.libraryName)], aggregateImage), /unexpected native framework/);
});

test('npm packs the selected framework despite the source checkout ignore rule', () => {
  const temporary = mkdtempSync(join(tmpdir(), 'prns-sdk-pack-fixture-'));
  try {
    const sourcePackage = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));
    writeFileSync(join(temporary, 'package.json'), JSON.stringify({
      name: 'prns-sdk-pack-fixture', version: '1.0.0', files: sourcePackage.files,
    }));
    writeFileSync(join(temporary, '.gitignore'), readFileSync(new URL('../.gitignore', import.meta.url)));
    const expected = [...frameworkFiles(aggregateImage.libraryName), file('arm64-v8a', aggregateImage.libraryName), { path: runtimePatch }];
    for (const { path } of expected) {
      const destination = join(temporary, path);
      mkdirSync(dirname(destination), { recursive: true });
      writeFileSync(destination, path.endsWith('Info.plist')
        ? '<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict/></plist>\n'
        : 'fixture native library\n');
    }
    const [packed] = JSON.parse(execFileSync('npm', ['pack', '--ignore-scripts', '--json', '--pack-destination', temporary], {
      cwd: temporary, encoding: 'utf8', timeout: 30000,
    }));
    assert.deepEqual(validatePackagedAppleImages(packed.files, aggregateImage, true).sort(),
      frameworkFiles(aggregateImage.libraryName).map(file => file.path).sort());
    validatePackagedImages(packed.files, aggregateImage);
    assert(packed.files.some(file => file.path === runtimePatch), 'the SDK archive must ship its runtime patch');
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
});


test('an aggregate consumer can select a staged SDK while retaining the shared test tooling', () => {
  const selected = consumerOptions(['--aggregate', '--sdk', '/consumer/target/sdk', '--android']);
  assert.equal(selected.sdk, '/consumer/target/sdk');
  assert.equal(selected.aggregate, true);
  assert.equal(selected.android, true);
});


const runtimePackages = () => ['core', 'react-native'].map(name => ({
  name: `@ubjs/${name}`, version: '0.31.0-6',
  archive: `https://registry.npmjs.org/@ubjs/${name}/-/${name}-0.31.0-6.tgz`,
  integrity: `sha512-${'a'.repeat(86)}==`, sha256: 'a'.repeat(64),
  license_notice: 'MPL notice', upstream: {},
}));

test('detached runtimes select exact official registry packages rather than copied archives', () => {
  const packages = runtimePackages();
  assert.deepEqual(consumerRuntimeDependencies(packages), {
    '@ubjs/core': '0.31.0-6', '@ubjs/react-native': '0.31.0-6',
  });
  assert.throws(() => consumerRuntimeDependencies(packages.slice(0, 1)), /both recorded/);
  assert.throws(() => consumerRuntimeDependencies([packages[0], packages[0]]), /duplicate runtime/);
  assert.throws(() => consumerRuntimeDependencies([{ ...packages[0], version: '^0.31.0' }, packages[1]]), /must be exact/);
  assert.throws(() => consumerRuntimeDependencies([{ ...packages[0], archive: 'vendor/runtime.tgz' }, packages[1]]), /official registry/);
  assert.throws(() => consumerRuntimeDependencies([{ ...packages[0], integrity: '' }, packages[1]]), /registry integrity/);
});

test('installed npm runtime versions, URLs and integrity must match the recorded inputs', () => {
  const packages = runtimePackages();
  const lock = { packages: Object.fromEntries(packages.map(runtime => [
    `node_modules/${runtime.name}`,
    { version: runtime.version, resolved: runtime.archive, integrity: runtime.integrity },
  ])) };
  validateInstalledRuntimes(lock, packages);
  assert.throws(() => validateInstalledRuntimes({ packages: {} }, packages), /missing installed runtime/);
  for (const field of ['version', 'resolved', 'integrity']) {
    const changed = structuredClone(lock);
    changed.packages['node_modules/@ubjs/react-native'][field] = 'changed';
    assert.throws(() => validateInstalledRuntimes(changed, packages), new RegExp(`${field} differs`));
  }
});

test('actual package attribution requires both files, exact selection and notice bytes', () => {
  const notice = 'native license material';
  const files = ['THIRD_PARTY_NOTICES.md', 'third-party-sources.json', runtimePatch].map(path => ({ path }));
  const record = { schemaVersion: 1, selection: defaultImage,
    notice_sha256: createHash('sha256').update(notice).digest('hex'),
    runtime_archives: { packages: runtimePackages() } };
  validatePackagedAttribution(files, defaultImage, record, notice);
  assert.throws(() => validatePackagedAttribution(files.slice(0, 1), defaultImage, record, notice), /missing packaged attribution/);
  assert.throws(() => validatePackagedAttribution(files.slice(0, 2), defaultImage, record, notice), /missing required runtime patch/);
  assert.throws(() => validatePackagedAttribution(files, aggregateImage, record, notice), /another native image/);
  assert.throws(() => validatePackagedAttribution(files, defaultImage, record, notice + 'changed'), /digest mismatch/);
  assert.throws(() => validatePackagedAttribution(files, defaultImage, { ...record, runtime_archives: {} }, notice), /missing runtime/);
});
