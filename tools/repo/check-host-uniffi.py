#!/usr/bin/env python3
"""Build the native image and qualify actual generated foreign bindings."""
from pathlib import Path
import argparse
import importlib.util
import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
IMAGE = ROOT / 'prns-host/bindings/uniffi/image'


def run(*args, env=None, timeout=None):
    subprocess.run([str(arg) for arg in args], cwd=ROOT, env=env, timeout=timeout, check=True)


def check_dependencies(env, connections=False):
    """Qualify the actual shipping image, independently of generation tooling."""
    sys.path.insert(0, str(ROOT / 'tools/uniffi'))
    import tooling

    target = 'aarch64-apple-darwin' if platform.machine() == 'arm64' else 'x86_64-apple-darwin'
    command = ['cargo', 'metadata', '--locked', '--format-version', '1',
               '--manifest-path', IMAGE / 'Cargo.toml', '--filter-platform', target]
    if connections:
        command.extend(['--features', 'connections'])
    metadata = json.loads(tooling.run(*command, env=env, capture=True))
    packages = {package['id']: package for package in metadata['packages']}
    nodes = {node['id']: node for node in metadata['resolve']['nodes']}
    roots = [key for key, package in packages.items()
             if package['name'] == 'prns-host-uniffi-image']
    if len(roots) != 1:
        raise ValueError('expected exactly one standalone host image')
    pending, selected = roots[:], set()
    while pending:
        package = pending.pop()
        if package not in selected:
            selected.add(package)
            pending.extend(dependency['pkg'] for dependency in nodes[package]['deps'])
    names = {packages[package]['name'] for package in selected}
    forbidden = {'prns-service-runtime', 'prns-service-runtime-uniffi', 'prns-service-runtime-uniffi-image',
                 'prns-contact-card', 'uniffi_bindgen', 'uniffi_bindgen_react_native'}
    unexpected = {name for name in names if name in forbidden or
                  name.startswith(('prns-identities', 'prns-groups', 'prns-lxmf'))}
    if unexpected:
        raise ValueError(f'independent host acquired unrelated dependencies: {sorted(unexpected)}')
    if any('applications' in Path(packages[package]['manifest_path']).parts
           for package in selected):
        raise ValueError('independent host depends on application code')
    for name in ('prns-host-uniffi-image', 'prns-host-uniffi', 'prns-host-runtime'):
        matches = [package for package in selected if packages[package]['name'] == name]
        if len(matches) != 1 or ('connections' in nodes[matches[0]]['features']) != connections:
            raise ValueError(f'{name} connection selection differs from requested profile')
    for name in ('prns-storage', 'redb'):
        if (name in names) != connections:
            raise ValueError(f'{name} selection differs from requested connections profile')


def swift_fixture(output, connections):
    if not connections:
        return IMAGE.parent / 'tests/swift_conformance.swift'
    fixture = output / 'connections_consumer.swift'
    fixture.write_text((IMAGE.parent / 'tests/connections_journey.swift').read_text() +
                       (IMAGE.parent / 'tests/connections_conformance.swift').read_text())
    return fixture


def qualify_swift(target, connections=False):
    # This consumes the shared native-only recipe; no React Native generator,
    # JavaScript install, or separate implementation of generation is involved.
    sys.path.insert(0, str(ROOT / 'tools/uniffi'))
    import tooling

    env = tooling.environment(target)
    env['CARGO_TARGET_DIR'] = str(target)
    check_dependencies(env)
    if connections:
        check_dependencies(env, connections=True)
    with tempfile.TemporaryDirectory(prefix='swift-conformance-', dir=target) as temporary:
        output = Path(temporary)
        recipe = tooling.BindingRecipe(
            manifest=IMAGE / 'Cargo.toml', package='prns-host-uniffi-image',
            library_name='prns_host_mobile', crate_dir=IMAGE,
            swift_dir=output, features=('uniffi-bindgen',) + (('connections',) if connections else ()),
        )
        for path, content in tooling.generated_files(recipe, None, env).items():
            path.write_text(content)
        executable = tooling.compile_swift_consumer(
            generated_sources=sorted(output.glob('*.swift')),
            module_maps=sorted(output.glob('*.modulemap')), module_name='PrnsHostBindings',
            native_library=target / 'debug/libprns_host_mobile.dylib',
            fixture=swift_fixture(output, connections), output_dir=output, env=env,
        )
        if connections:
            run(executable, env=env, timeout=120)
            return
        spec = importlib.util.spec_from_file_location('host_contract_generator', ROOT / 'tools/repo/generate-host-contract.py')
        contract = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(contract)
        schema = json.loads(contract.SCHEMA_PATH.read_text())
        run(executable, contract.semantic_fingerprint(schema), schema['productVersion'], env=env, timeout=120)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--language', choices=('python', 'swift'), default='python')
    parser.add_argument('--connections', action='store_true',
                        help='qualify standalone saved profiles through the real Swift host image')
    args = parser.parse_args(argv)
    if args.connections and args.language != 'swift':
        parser.error('--connections requires --language swift')
    if args.language == 'swift' and sys.platform != 'darwin':
        parser.error('Swift native conformance requires macOS and the Xcode command-line tools')
    run(sys.executable, ROOT / 'tools/repo/generate-host-contract.py', '--check')
    test = ['cargo', 'test', '--locked', '--manifest-path', IMAGE.parent / 'Cargo.toml', '--test', 'transport']
    if args.connections:
        test.extend(['--features', 'connections'])
    run(*test)
    target = Path(os.environ.get('CARGO_TARGET_DIR', IMAGE / 'target'))
    if not target.is_absolute():
        target = ROOT / target
    target.mkdir(parents=True, exist_ok=True)
    if args.language == 'swift':
        qualify_swift(target, connections=args.connections)
        return
    run('cargo', 'build', '--locked', '--manifest-path', IMAGE / 'Cargo.toml',
        '--features', 'uniffi-bindgen', '--lib', '--bin', 'uniffi-bindgen')
    extension = '.dylib' if sys.platform == 'darwin' else '.dll' if sys.platform == 'win32' else '.so'
    library = target / 'debug' / (('' if sys.platform == 'win32' else 'lib') + 'prns_host_mobile' + extension)
    bindgen = target / 'debug' / ('uniffi-bindgen.exe' if sys.platform == 'win32' else 'uniffi-bindgen')
    with tempfile.TemporaryDirectory(prefix='foreign-conformance-', dir=target) as generated:
        output = Path(generated)
        run(bindgen, 'generate', '--library', library, '--language', 'python', '--out-dir', output, '--no-format')
        if sys.platform == 'win32': shutil.copy2(library, output / library.name)
        else: (output / library.name).symlink_to(library)
        run(sys.executable, IMAGE.parent / 'tests/foreign_conformance.py', output)
        run(sys.executable, IMAGE.parent / 'tests/remote_control_foreign_conformance.py', output)


if __name__ == '__main__':
    main()
