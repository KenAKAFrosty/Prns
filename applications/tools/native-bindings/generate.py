#!/usr/bin/env python3
"""Generate all selected app namespaces and stage the SDK from the same image."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys

HERE = Path(__file__).resolve().parent
APPLICATIONS = HERE.parents[1]
ROOT = APPLICATIONS.parent
CRATE = APPLICATIONS / 'prns/chat-app'
BINDINGS = APPLICATIONS / 'prns/bindings'
SDK_PACKAGE = APPLICATIONS / 'target/react-native-sdk'
IMAGE = HERE / 'image.json'
OWNERSHIP = HERE / 'outputs.json'
sys.path.insert(0, str(ROOT / 'tools/uniffi'))
import tooling

spec = importlib.util.spec_from_file_location(
    'prns_sdk_generation', ROOT / 'prns-react-native/tools/generate.py')
sdk = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sdk)


def app_api_version():
    version = json.loads(IMAGE.read_text())['appApiVersion']
    if type(version) is not int or not 1 <= version <= 0xffff_ffff:
        raise ValueError('appApiVersion must be a positive u32')
    return version


def assemble(files, version):
    """Namespace assembly only; never inspect or select domain methods/types."""
    files = dict(files)
    typescript = BINDINGS / 'typescript'
    # The composition owns entrypoints and borrows the SDK's host initialization.
    # The upstream barrel eagerly initializes every namespace, including the host.
    files.pop(typescript / 'index.ts', None)
    host = typescript / 'prns_host_uniffi.ts'
    host_ffi = typescript / 'prns_host_uniffi-ffi.ts'
    if not {host, host_ffi, typescript / 'prns_chat_app.ts'}.issubset(files):
        raise ValueError('the selected image omitted its app or canonical host namespace')
    del files[host_ffi]
    files[host] = (
        '// Generated canonical host converter import. The SDK owns the implementation.\n'
        'export * from "personal-rns-expo/bindings/values";\n'
        'export { default } from "personal-rns-expo/bindings/values";\n')
    namespaces = sorted(path.stem for path in files
                        if path.parent == typescript and path.suffix == '.ts'
                        and not path.stem.endswith('-ffi'))
    if not all(re.fullmatch(r'[a-z][a-z0-9_]*', name) for name in namespaces):
        raise ValueError('generated namespace is not a TypeScript export identifier')
    header = '// Generated namespace assembly. Domain APIs remain unmodified.\n'
    exports = ''.join(
        f'export * as {name} from "./typescript/{name}";\n' for name in namespaces)
    files[BINDINGS / 'index.ts'] = header + exports
    native = [header, 'import "personal-rns-expo/bindings";\n',
              'import "@ubjs/react-native";\n']
    modules = [name for name in namespaces if name != 'prns_host_uniffi']
    for name in modules:
        native.append(f'import {name} from "./typescript/{name}";\n')
    native.append('import { appApiVersion } from "./typescript/prns_chat_app";\n\n')
    native.extend(f'{name}.initialize();\n' for name in modules)
    native.append(f'if (appApiVersion() !== {version}) {{\n'
                  '  throw new Error("PRNS app API version mismatch; rebuild the native application");\n'
                  '}\n')
    # Metro resolves './index' back to this native entrypoint. Reuse the owning
    # namespace exports directly so native initialization never imports itself.
    native.append(exports)
    files[BINDINGS / 'index.native.ts'] = ''.join(native)
    return files


def generated_files(cli, env):
    provider = sdk.image_provider(IMAGE)
    recipe = tooling.BindingRecipe(
        manifest=provider['manifest'], package=provider['package'],
        library_name=provider['libraryName'], crate_dir=provider['crateDirectory'],
        typescript_dir=BINDINGS / 'typescript', swift_dir=BINDINGS / 'swift',
        kotlin_dir=BINDINGS / 'kotlin',
        features=tuple([*provider.get('features', []),
                        *provider.get('generationFeatures', [])]),
    )
    return assemble(tooling.generated_files(recipe, cli, env), app_api_version())


def generate(cli, env, check=False, stage_only=False):
    # Validate the entire owned set before creating generated outputs.
    tooling.existing_outputs(*tooling.ownership(APPLICATIONS, OWNERSHIP))
    provider = sdk.image_provider(IMAGE)
    sdk.selected_destination(provider, SDK_PACKAGE)
    files = generated_files(cli, env)
    tooling.synchronize_outputs(files, check or stage_only, APPLICATIONS, OWNERSHIP)
    # The SDK's canonical host converter comes from exactly this aggregate, with
    # the same Cargo features and ABI checksums as the ordinary domain clients.
    sdk.generate(provider, cli, env, check, SDK_PACKAGE)


def execute(cli, env, args):
    if args.command == 'stage':
        generate(cli, env, stage_only=True)
    elif args.command == 'generate':
        generate(cli, env, check=args.check)
    else:
        generate(cli, env)
        sdk.build(sdk.image_provider(IMAGE), cli, env, args, SDK_PACKAGE)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cache-dir', type=Path,
                        default=Path(os.environ.get('PRNS_UBRN_CACHE', ROOT / 'target/ubrn-tooling')))
    commands = parser.add_subparsers(dest='command', required=True)
    generate_command = commands.add_parser('generate')
    generate_command.add_argument('--check', action='store_true')
    commands.add_parser('stage', help='check tracked bindings and materialize the ignored SDK package')
    for name in ('ios', 'android'):
        command = commands.add_parser(name)
        command.add_argument('--release', action='store_true')
        command.add_argument('--targets')
        if name == 'ios':
            command.add_argument('--sim-only', action='store_true')
    args = parser.parse_args()
    try:
        tooling.existing_outputs(*tooling.ownership(APPLICATIONS, OWNERSHIP))
        env = tooling.environment(APPLICATIONS / 'target')
        cache = args.cache_dir.expanduser().resolve()
        cache.mkdir(parents=True, exist_ok=True)
        cli = tooling.generator(cache, env)
        execute(cli, env, args)
    except (ValueError, KeyError, FileNotFoundError, subprocess.CalledProcessError) as error:
        parser.exit(1, f'app native bindings: {error}\n')


if __name__ == '__main__':
    main()
