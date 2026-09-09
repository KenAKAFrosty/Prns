#!/usr/bin/env python3
"""Generate/check the app bindings, or build their single shared mobile image."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
APPLICATIONS = HERE.parents[1]
CRATE = APPLICATIONS / 'prns/native-composition'
BINDINGS = CRATE / 'bindings'
VENDOR_TOOL = APPLICATIONS / 'tools/ubrn-vendor/vendor.py'
spec = importlib.util.spec_from_file_location('prns_ubrn_vendor', VENDOR_TOOL)
vendor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vendor)


def run(*args, cwd=APPLICATIONS, env=None, capture=False):
    result = subprocess.run([str(arg) for arg in args], cwd=cwd, env=env, check=True,
                            stdout=subprocess.PIPE if capture else None, text=True)
    return result.stdout if capture else None


def environment():
    env = dict(os.environ)
    env.setdefault('CARGO_TARGET_DIR', str(APPLICATIONS / 'target'))
    env.setdefault('CARGO_BUILD_JOBS', '4')
    env.setdefault('RUSTUP_TOOLCHAIN', 'stable')
    if sys.platform == 'darwin':
        clang = run('xcrun', '--find', 'clang', capture=True).strip()
        env.update(PATH=str(Path(clang).parent) + os.pathsep + env['PATH'], CC=clang,
                   CXX=run('xcrun', '--find', 'clang++', capture=True).strip(),
                   AR=run('xcrun', '--find', 'ar', capture=True).strip(),
                   RANLIB=run('xcrun', '--find', 'ranlib', capture=True).strip(), LIBRARY_PATH='')
        for key in ('SDKROOT', 'CFLAGS', 'CXXFLAGS', 'CPPFLAGS'):
            env.pop(key, None)
    return env


def generator(cache, env):
    source, _ = vendor.prepare(cache, vendor.inputs())
    cli_env = dict(env, CARGO_TARGET_DIR=str(cache / 'cli-target'))
    run('cargo', 'build', '--locked', '--no-default-features', '-p',
        'uniffi-bindgen-react-native', cwd=source, env=cli_env)
    return cache / 'cli-target/debug/uniffi-bindgen-react-native'


def normalize(text):
    return '\n'.join(line.rstrip() for line in text.splitlines()).rstrip() + '\n'


def generated_files(cli, env):
    run('cargo', 'build', '--locked', '--manifest-path', APPLICATIONS / 'Cargo.toml',
        '-p', 'prns-app-native', '--features', 'host-test,uniffi-bindgen',
        '--lib', '--bin', 'uniffi-bindgen', '--bin', 'export_contract', env=env)
    target = Path(env['CARGO_TARGET_DIR']).resolve()
    suffix = 'dylib' if sys.platform == 'darwin' else 'so'
    library = target / f'debug/libprns_app.{suffix}'
    with tempfile.TemporaryDirectory(prefix='prns-generated-', dir=target) as temporary:
        temp = Path(temporary)
        ts = temp / 'typescript'
        native = temp / 'native'
        run(cli, 'generate', 'jsi2', 'bindings', '--library', library,
            '--ts-dir', ts, '--lib-name', 'prns_app', '--no-format', cwd=CRATE, env=env)
        run(target / 'debug/uniffi-bindgen', 'generate', '--library', library,
            '--config', CRATE / 'uniffi.toml', '--language', 'swift', '--language', 'kotlin',
            '--no-format', '--out-dir', native, env=env)
        files = {BINDINGS / 'typescript' / file.name: normalize(file.read_text())
                 for file in ts.glob('*.ts')}
        for file in native.iterdir():
            if file.is_file():
                files[APPLICATIONS / 'sdk/expo/ios/generated' / file.name] = normalize(file.read_text())
        for file in native.rglob('*.kt'):
            files[APPLICATIONS / 'sdk/expo/android/src/main/java' / file.relative_to(native)] = normalize(file.read_text())
        fingerprints = json.loads(run(target / 'debug/export_contract', '--fingerprints', env=env, capture=True))
        files[BINDINGS / 'typescript/binding-contract.generated.ts'] = (
            '// Generated from the Rust app and canonical Host contract identifiers.\n'
            f'export const NATIVE_CONTRACT_FINGERPRINT = {json.dumps(fingerprints["app"])};\n'
            f'export const HOST_CONTRACT_FINGERPRINT = {json.dumps(fingerprints["host"])};\n')
        return files


def generate(cli, env, check):
    run('python3', HERE / 'host_contract.py', *(['--check'] if check else []), env=env)
    stale = []
    for path, text in generated_files(cli, env).items():
        if check:
            if not path.exists() or path.read_text() != text:
                stale.append(str(path.relative_to(APPLICATIONS)))
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
    if stale:
        raise ValueError('generated bindings differ; run npm run api:generate:\n' + '\n'.join(stale))


def build(cli, env, args):
    if args.command == 'android':
        expected = vendor.inputs()['toolchain']['androidNdk']
        sdk = Path(env.get('ANDROID_HOME') or env.get('ANDROID_SDK_ROOT') or
                   (Path.home() / ('Library/Android/sdk' if sys.platform == 'darwin' else 'Android/Sdk')))
        ndk = Path(env.get('ANDROID_NDK_HOME') or sdk / 'ndk' / expected).expanduser().resolve()
        properties = dict(tuple(part.strip() for part in line.split('=', 1))
                          for line in (ndk / 'source.properties').read_text().splitlines() if '=' in line)
        if properties.get('Pkg.Revision') != expected:
            raise ValueError(f'Android bindings require NDK {expected}; set ANDROID_NDK_HOME to that installation')
        env = dict(env, ANDROID_NDK_HOME=str(ndk), NDK_HOME=str(ndk))
    command = [cli, 'build', 'jsi2', args.command, '--config', BINDINGS / 'ubrn.config.yaml']
    if args.release:
        command.append('--release')
    if args.targets:
        command.extend(['--targets', args.targets])
    if args.command == 'ios' and args.sim_only:
        command.append('--sim-only')
    if args.command == 'ios' and not args.release:
        text = (BINDINGS / 'ubrn.config.yaml').read_text()
        text = text.replace('apple,uniffi-bindings', 'apple,uniffi-bindings,ios-restoration-probe')
        with tempfile.NamedTemporaryFile(mode='w', prefix='.ubrn-build-', suffix='.yaml', dir=BINDINGS) as config:
            config.write(text)
            config.flush()
            command[command.index('--config') + 1] = config.name
            run(*command, cwd=BINDINGS, env=env)
    else:
        run(*command, cwd=BINDINGS, env=env)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cache-dir', type=Path,
                        default=Path(os.environ.get('PRNS_UBRN_CACHE', APPLICATIONS / 'target/ubrn-tooling')))
    commands = parser.add_subparsers(dest='command', required=True)
    gen = commands.add_parser('generate')
    gen.add_argument('--check', action='store_true')
    for name in ('ios', 'android'):
        native = commands.add_parser(name)
        native.add_argument('--release', action='store_true')
        native.add_argument('--targets')
        if name == 'ios':
            native.add_argument('--sim-only', action='store_true')
    args = parser.parse_args()
    try:
        env = environment()
        cache = args.cache_dir.expanduser().resolve()
        cache.mkdir(parents=True, exist_ok=True)
        cli = generator(cache, env)
        if args.command == 'generate':
            generate(cli, env, args.check)
        else:
            build(cli, env, args)
    except (ValueError, FileNotFoundError, subprocess.CalledProcessError) as error:
        parser.exit(1, f'generated bindings: {error}\n')
