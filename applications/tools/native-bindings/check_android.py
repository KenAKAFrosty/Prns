#!/usr/bin/env python3
"""Build/test the selected Android app and inspect its unsigned arm64 Debug APK."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET
import zipfile

import android_maven

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
APPLICATIONS = ROOT / 'applications'
APP = APPLICATIONS / 'prns/app'
SDK_PACKAGE = APPLICATIONS / 'target/react-native-sdk'
APK = APP / 'android/app/build/outputs/apk/debug/app-debug-unsigned.apk'
MINIMUM_FREE = 5 * 1024 ** 3


def require(condition, message):
    if not condition:
        raise ValueError(message)


def disk_guard():
    for path in (ROOT, Path(tempfile.gettempdir())):
        require(shutil.disk_usage(path).free >= MINIMUM_FREE,
                f'Android qualification requires at least 5 GiB free on {path}')


def run(name, command, cwd, env, logs):
    disk_guard()
    print(f'Android qualification: {name}', flush=True)
    log = logs / f'{name}.log'
    with log.open('w') as output:
        process = subprocess.Popen([str(value) for value in command], cwd=cwd, env=env,
                                   stdout=output, stderr=subprocess.STDOUT,
                                   start_new_session=True)
        try:
            while process.poll() is None:
                disk_guard()
                time.sleep(2)
        except BaseException:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            raise
    require(process.returncode == 0, f'{name} failed ({process.returncode}); see {log}')


def unsigned_init():
    # This affects only this qualification invocation, not the product project.
    tests = ROOT / 'prns-react-native/android/src/test/java'
    return '''allprojects { project ->
  project.plugins.withId('com.android.application') {
    project.extensions.getByName('androidComponents').finalizeDsl { android ->
      android.buildTypes.getByName('debug').signingConfig = null
    }
  }
  if (project.name == 'personal-rns-expo') {
    project.plugins.withId('com.android.library') {
      project.afterEvaluate {
        project.extensions.getByName('android').sourceSets.getByName('test').java.srcDir(
          new File(System.getenv('PRNS_QUALIFICATION_SDK_TESTS')))
      }
    }
  }
}
''' + android_maven.gradle_init(), str(tests)


def test_counts(directory):
    files = sorted(directory.glob('TEST-*.xml'))
    require(bool(files), f'No completed test reports at {directory}')
    totals = {name: 0 for name in ('tests', 'failures', 'errors', 'skipped')}
    for file in files:
        suite = ET.parse(file).getroot()
        for name in totals:
            totals[name] += int(suite.get(name, '0'))
    require(totals['tests'] > 0 and not totals['failures'] and not totals['errors'],
            f'Failed or empty test run: {directory}: {totals}')
    return totals


def inspect(sdk, ndk, logs):
    require(APK.is_file(), f'Unsigned APK missing: {APK}')
    builds = sorted((path for path in (sdk / 'build-tools').iterdir()
                     if re.fullmatch(r'\d+\.\d+\.\d+', path.name)),
                    key=lambda path: tuple(map(int, path.name.split('.'))))
    require(bool(builds), 'Install Android build-tools')
    build_tools = builds[-1]
    readelfs = list((ndk / 'toolchains/llvm/prebuilt').glob('*/bin/llvm-readelf'))
    require(len(readelfs) == 1, 'Expected one installed NDK llvm-readelf')
    subprocess.run([str(build_tools / 'zipalign'), '-c', '-P', '16', '4', str(APK)], check=True)
    with zipfile.ZipFile(APK) as archive:
        libraries = [name for name in archive.namelist() if name.startswith('lib/')]
        require({name.split('/')[1] for name in libraries} == {'arm64-v8a'}, 'Unexpected APK ABI')
        require([name for name in libraries if 'prns' in name] == ['lib/arm64-v8a/libprns_chat_app.so'],
                'APK must contain exactly one PRNS image')
        require(not any(name.startswith('META-INF/') and name.endswith(('.RSA', '.SF', '.DSA', '.EC'))
                        for name in archive.namelist()), 'APK contains signing entries')
        with APK.open('rb') as source:
            source.seek(archive.start_dir - 16)
            require(source.read(16) != b'APK Sig Block 42', 'APK contains a signing block')
        with tempfile.TemporaryDirectory(prefix='apk-image-', dir=logs) as temporary:
            image = Path(temporary) / 'libprns_chat_app.so'
            image.write_bytes(archive.read('lib/arm64-v8a/libprns_chat_app.so'))
            elf = subprocess.check_output([str(readelfs[0]), '-l', '-d', str(image)], text=True)
    loads = [line for line in elf.splitlines() if line.strip().startswith('LOAD')]
    require(bool(loads) and all(line.split()[-1] == '0x4000' for line in loads),
            'Packaged PRNS image lacks 16 KiB LOAD alignment')
    require('Library soname: [libprns_chat_app.so]' in elf, 'Incorrect packaged image SONAME')
    signature = subprocess.run([str(build_tools / 'apksigner'), 'verify', str(APK)],
                               capture_output=True, text=True)
    require(signature.returncode != 0, 'APK unexpectedly verifies as signed')
    (logs / 'unsigned-verification.log').write_text(signature.stdout + signature.stderr)
    analyzer = sdk / 'cmdline-tools/latest/bin/apkanalyzer'
    manifest = ET.fromstring(subprocess.check_output([str(analyzer), 'manifest', 'print', str(APK)]))
    android = '{http://schemas.android.com/apk/res/android}'
    require(manifest.get('package') == 'rs.reticulum.prns.dev', 'Unexpected application ID')
    require(manifest.find('uses-sdk').get(android + 'minSdkVersion') == '29', 'Unexpected minimum SDK')
    app = manifest.find('application')
    require(app.get(android + 'allowBackup') == 'false', 'Private installation permits backup')
    service = next((item for item in app.findall('service')
                    if item.get(android + 'name') == 'rs.reticulum.prns.app.platform.PrnsAppService'), None)
    require(service is not None and service.get(android + 'exported') == 'false'
            and service.get(android + 'foregroundServiceType') in {'connectedDevice', '0x10'}
            and service.get(android + 'stopWithTask') == 'false', 'Incorrect runtime service manifest')
    with APK.open('rb') as source:
        digest = hashlib.file_digest(source, 'sha256').hexdigest()
    report = {'apk': str(APK), 'bytes': APK.stat().st_size,
              'sha256': digest,
              'abi': 'arm64-v8a', 'signed': False, 'image': 'libprns_chat_app.so',
              'sdk_tests': test_counts(SDK_PACKAGE / 'android/build/test-results/testDebugUnitTest'),
              'product_tests': test_counts(APPLICATIONS / 'prns/platform/android/build/test-results/testDebugUnitTest'),
              'maven_runtime': json.loads((logs / 'maven-report.json').read_text())}
    (logs / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cache-dir', type=Path, default=os.environ.get('PRNS_UBRN_CACHE'))
    parser.add_argument('--offline', action='store_true', help='require already cached Rust/Gradle dependencies')
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--inspect-only', action='store_true', help='inspect existing APK/test/dependency reports without rebuilding')
    mode.add_argument('--dependencies-only', action='store_true', help='resolve/check debug and release runtime dependencies without rebuilding or changing the APK/report')
    parser.add_argument('--logs', type=Path, default=APPLICATIONS / 'target/android-qualification')
    args = parser.parse_args()
    require(os.name == 'posix', 'This qualification recipe requires macOS or Linux')
    sdk = Path(os.environ['ANDROID_HOME']).expanduser().resolve()
    version = json.loads((ROOT / 'tools/uniffi/react-native.json').read_text())['toolchain']['androidNdk']
    ndk = Path(os.environ.get('ANDROID_NDK_HOME', sdk / 'ndk' / version)).expanduser().resolve()
    require(f'Pkg.Revision = {version}' in (ndk / 'source.properties').read_text(),
            f'Install the pinned Android NDK {version}')
    disk_guard()
    args.logs.mkdir(parents=True, exist_ok=True)
    if not args.inspect_only:
        if not args.dependencies_only:
            require(args.cache_dir is not None, 'Set PRNS_UBRN_CACHE or --cache-dir')
            require(all(os.environ.get(key) for key in ('CARGO_TARGET_DIR', 'PRNS_UBRN_TARGET_DIR')),
                    'Set CARGO_TARGET_DIR and PRNS_UBRN_TARGET_DIR to the shared build cache')
        env = dict(os.environ, ANDROID_HOME=str(sdk), ANDROID_NDK_HOME=str(ndk),
                   CI='1', EXPO_NO_TELEMETRY='1', PRNS_APP_VARIANT='development',
                   CARGO_INCREMENTAL='0', CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_TEST_DEBUG='0')
        if args.offline:
            env['CARGO_NET_OFFLINE'] = 'true'
        if not args.dependencies_only:
            run('native-image', [sys.executable, HERE / 'generate.py', '--cache-dir', args.cache_dir.expanduser().resolve(),
                                'android', '--targets', 'arm64-v8a'], ROOT, env, args.logs)
            package = APP / 'package.json'
            before = package.read_bytes()
            run('prebuild', ['node', '../../node_modules/expo/bin/cli', 'prebuild', '--platform', 'android',
                            '--no-install', '--skip-dependency-update', 'react,react-native,expo'], APP, env, args.logs)
            require(package.read_bytes() == before, 'Expo prebuild changed the product package manifest')
        init, env['PRNS_QUALIFICATION_SDK_TESTS'] = unsigned_init()
        init_file = args.logs / 'unsigned.gradle'
        init_file.write_text(init)
        env['PRNS_ANDROID_MAVEN_REPORT'] = str((args.logs / 'maven-runtime.json').resolve())
        tasks = [android_maven.TASK]
        if not args.dependencies_only:
            tasks += [':personal-rns-expo:testDebugUnitTest', ':prns-app-platform:testDebugUnitTest', ':app:assembleDebug']
        run('gradle', ['./gradlew', '--no-daemon', *(['--offline'] if args.offline else []), '--max-workers=2',
                       '-Dorg.gradle.jvmargs=-Xmx4096m -XX:MaxMetaspaceSize=1024m',
                       '-Pkotlin.compiler.execution.strategy=in-process', '--init-script', init_file.resolve(),
                       '-PreactNativeArchitectures=arm64-v8a', *tasks], APP / 'android', env, args.logs)
        android_maven.check_report(args.logs / 'maven-runtime.json', args.logs / 'maven-report.json')
        if args.dependencies_only:
            print(f'Android dependency inventory passed; report: {args.logs / "maven-report.json"}', flush=True)
            return
    if args.inspect_only:
        android_maven.check_report(args.logs / 'maven-runtime.json', args.logs / 'maven-report.json')
    inspect(sdk, ndk, args.logs)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        raise SystemExit(f'Android qualification: {error}')
