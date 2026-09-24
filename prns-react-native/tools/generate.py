#!/usr/bin/env python3
"""Generate the general Expo SDK from exactly one selected native image."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

PACKAGE = Path(__file__).resolve().parents[1]
ROOT = PACKAGE.parent
sys.path.insert(0, str(PACKAGE / 'tools'))
sys.path.insert(0, str(ROOT / 'tools/uniffi'))
import package_staging
import tooling


def image_provider(path):
    document = json.loads(path.read_text())
    if document.get('schemaVersion') != 1:
        raise ValueError('unsupported native image provider version')
    providers = document.get('providers', [])
    if len(providers) != 1:
        raise ValueError('select exactly one native image provider: default OR aggregate')
    provider = dict(providers[0])
    for key in ('id', 'package', 'libraryName'):
        if not isinstance(provider.get(key), str) or not re.fullmatch(r'[a-z][a-z0-9_-]*', provider[key]):
            raise ValueError(f'invalid image provider {key}')
    if not re.fullmatch(r'[a-z][a-z0-9_]*', provider['libraryName']):
        raise ValueError('image libraryName must be a Rust library identifier')
    for key in ('features', 'generationFeatures'):
        if not isinstance(provider.get(key, []), list) or not all(isinstance(value, str) for value in provider.get(key, [])):
            raise ValueError(f'image {key} must be a list of strings')
    for key in ('manifest', 'crateDirectory'):
        provider[key] = (path.parent / provider[key]).resolve()
        if not provider[key].exists():
            raise ValueError(f'image provider {key} does not exist: {provider[key]}')
    return provider


def selection_files(provider, destination=PACKAGE):
    image = provider['libraryName']
    selection = {'schemaVersion': 1, 'provider': provider['id'], 'libraryName': image}
    return {
        destination / 'native-image.json': json.dumps(selection, indent=2) + '\n',
        destination / 'src/generated/native-image.generated.ts': f'// Generated image selection.\nexport const NATIVE_IMAGE = {json.dumps(image)};\n',
        destination / 'ios/NativeImage.generated.swift': f'// Generated image selection.\nenum PrnsNativeImage {{ static let name = {json.dumps(image)} }}\n',
        destination / 'android/src/main/java/rs/reticulum/prns/expo/NativeImage.generated.kt': f'// Generated image selection.\npackage rs.reticulum.prns.expo\ninternal object PrnsNativeImage {{ const val name = {json.dumps(image)} }}\n',
    }


def selected_destination(provider, destination):
    destination = package_staging.destination_path(PACKAGE, destination)
    default = json.loads((PACKAGE / 'providers/default.json').read_text())['providers'][0]
    if destination == PACKAGE and any(provider[key] != default[key] for key in ('id', 'libraryName')):
        raise ValueError('an aggregate provider requires a separate --destination')
    selection = destination / 'native-image.json'
    if destination != PACKAGE and selection.exists():
        existing = json.loads(selection.read_text())
        if existing['libraryName'] != provider['libraryName']:
            raise ValueError('SDK destination already selects another image; choose a clean destination')
    return destination


def generate(provider, cli, env, check, destination=PACKAGE):
    destination = selected_destination(provider, destination)
    tooling.existing_outputs(*tooling.ownership(destination, PACKAGE / 'outputs.json'))
    # The selected image determines the name for JSI and JNA alike. Canonical
    # namespace and semantic conversions are independent of the image provider.
    recipe = tooling.BindingRecipe(
        manifest=provider['manifest'], package=provider['package'],
        library_name=provider['libraryName'], crate_dir=provider['crateDirectory'],
        typescript_dir=destination / 'src/generated', swift_dir=destination / 'ios/generated',
        kotlin_dir=destination / 'android/generated', uniffi_config=None,
        features=tuple([*provider.get('features', []), *provider.get('generationFeatures', [])]),
    )
    files = tooling.generated_files(recipe, cli, env)
    # Only the shared SDK namespace belongs in this package. An aggregate's
    # product namespaces are emitted by its own recipe, importing this converter.
    sdk_names = {'prns_host_uniffi.ts', 'prns_host_uniffi-ffi.ts',
                 'prns_host_uniffi.swift', 'prns_host_uniffiFFI.h',
                 'prns_host_uniffiFFI.modulemap', 'prns_host_uniffi.kt',
                 'PrnsHostBindings.swift', 'PrnsHostBindingsFFI.h', 'PrnsHostBindingsFFI.modulemap'}
    files = {path: content for path, content in files.items() if path.name in sdk_names}
    for adapter in ('host-adapter.generated.ts', 'remote-control-adapter.generated.ts'):
        files[destination / 'src/generated' / adapter] = (
            ROOT / 'prns-host/bindings/uniffi/typescript' / adapter).read_text()
    transport = (ROOT / 'prns-host/bindings/uniffi/src/transport.generated.rs').read_text()
    fingerprint = re.search(r'HOST_SEMANTIC_FINGERPRINT: &str =\s*"([0-9a-f]+)"', transport)
    if fingerprint is None:
        raise ValueError('generated host transport lacks its semantic fingerprint')
    files[destination / 'src/generated/contract.generated.ts'] = (
        '// Generated canonical host fingerprint.\n'
        f'export const HOST_SEMANTIC_FINGERPRINT = {json.dumps(fingerprint.group(1))};\n')
    files.update(selection_files(provider, destination))
    for name in ('LICENSE-MIT', 'LICENSE-APACHE'):
        files[destination / name] = (ROOT / name).read_text()
    package_staging.synchronize(PACKAGE, destination, check)
    tooling.synchronize_outputs(files, check, destination, PACKAGE / 'outputs.json')


def build(provider, cli, env, args, destination=PACKAGE):
    destination = selected_destination(provider, destination)
    # Keep the temporary configuration beside package.json so ubrn writes the
    # selected framework/jniLibs into this package, irrespective of Rust location.
    text = f'''rust:
  directory: {json.dumps(str(provider['crateDirectory']))}
  manifestPath: {json.dumps(str(provider['crateDirectory'] / 'Cargo.toml'))}
bindings:
  ts: src/generated
jsi2:
  ts: src/generated
  androidTargets: [arm64-v8a, x86_64]
  minIosVersion: "18.0"
  bundleIdPrefix: rs.reticulum.prns.host
ios:
  targets: [aarch64-apple-ios, aarch64-apple-ios-sim]
  cargoExtras: {json.dumps(['--locked', '--lib', *(['--features', ','.join(provider['features'])] if provider.get('features') else [])])}
android:
  apiLevel: 29
  cargoExtras: {json.dumps(['--locked', '--lib', *(['--features', ','.join(provider['features'])] if provider.get('features') else [])])}
'''
    with tempfile.NamedTemporaryFile(mode='w', suffix='.yaml', prefix='.ubrn-', dir=destination) as config:
        config.write(text)
        config.flush()
        tooling.build_mobile(cli, env, args, Path(config.name))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--provider', type=Path, default=PACKAGE / 'providers/default.json')
    parser.add_argument('--destination', type=Path, default=PACKAGE,
                        help='disposable package destination; required for aggregate providers')
    parser.add_argument('--cache-dir', type=Path, default=Path(os.environ.get('PRNS_UBRN_CACHE', ROOT / 'target/ubrn-tooling')))
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
        provider = image_provider(args.provider.resolve())
        args.destination = selected_destination(provider, args.destination)
        env = tooling.environment(ROOT / 'target/prns-react-native')
        cache = args.cache_dir.expanduser().resolve()
        cache.mkdir(parents=True, exist_ok=True)
        cli = tooling.generator(cache, env)
        generate(provider, cli, env, args.command == 'generate' and args.check, args.destination)
        if args.command != 'generate':
            build(provider, cli, env, args, args.destination)
    except (ValueError, KeyError, FileNotFoundError, subprocess.CalledProcessError) as error:
        parser.exit(1, f'PRNS Expo bindings: {error}\n')


if __name__ == '__main__':
    main()
