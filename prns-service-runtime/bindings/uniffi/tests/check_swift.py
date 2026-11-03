#!/usr/bin/env python3
"""Qualify composed construction with ordinary generated clients in one image."""
import argparse
import importlib.util
import json
from pathlib import Path
import platform
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[4]
BINDINGS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tools/uniffi'))
import tooling


def check_inventory(headers, connections=False):
    prefix = 'uniffi_prns_service_runtime_uniffi_fn_'
    expected = {prefix + name for name in (
        'constructor_serviceruntime_create_new', 'constructor_serviceruntime_open_existing',
        'constructor_serviceruntime_create_new_offline', 'constructor_serviceruntime_open_existing_offline',
        'method_serviceruntime_ready', 'method_serviceruntime_status',
        'method_serviceruntime_network_ready', 'method_serviceruntime_network_status',
        'method_serviceruntime_start_networking', 'method_serviceruntime_stop_networking',
        'method_serviceruntime_announcements',
        'method_serviceruntime_identities', 'method_serviceruntime_groups',
        'method_serviceruntime_host', 'method_serviceruntime_shutdown',
        'method_serviceruntimeclose_status', 'method_serviceruntimeclose_wait',
        'method_serviceruntimenetworkchange_status', 'method_serviceruntimenetworkchange_wait')}
    if connections:
        expected.update(prefix + name for name in (
            'constructor_serviceruntime_create_new_with_connections',
            'constructor_serviceruntime_open_existing_with_connections',
            'method_serviceruntime_connection_binding'))
    actual = set(re.findall(r'\b(uniffi_prns_service_runtime_uniffi_fn_(?:constructor|method|func)_\w+)\s*\(', headers))
    if actual != expected:
        raise ValueError(f'construction exposure changed: missing={sorted(expected-actual)}, extra={sorted(actual-expected)}')
    # Domain behavior is emitted under its owning namespace, never this factory.
    for name in ('uniffi_prns_identities_fn_method_identitycatalogclient_create_or_recover',
                 'uniffi_prns_groups_fn_method_client_submit',
                 'uniffi_prns_host_uniffi_fn_method_hostclienthandle_snapshot'):
        if name not in headers:
            raise ValueError(f'missing canonical namespace method: {name}')


def check_dependencies(env, features):
    target = 'aarch64-apple-darwin' if platform.machine() == 'arm64' else 'x86_64-apple-darwin'
    command = ['cargo', 'metadata', '--locked', '--format-version', '1', '--manifest-path',
               BINDINGS / 'image/Cargo.toml', '--filter-platform', target]
    if features:
        command.extend(['--features', ','.join(features)])
    metadata = json.loads(tooling.run(*command, env=env, capture=True))
    packages = {package['id']: package for package in metadata['packages']}
    nodes = {node['id']: node for node in metadata['resolve']['nodes']}
    pending = [key for key, package in packages.items() if package['name'] == 'prns-service-runtime-uniffi-image']
    selected = set()
    while pending:
        package = pending.pop()
        if package not in selected:
            selected.add(package)
            pending.extend(dep['pkg'] for dep in nodes[package]['deps'])
    names = {packages[package]['name'] for package in selected}
    forbidden = {'prns-lxmf', 'prns-lxmf-wire', 'prns-react-native', 'uniffi_bindgen', 'uniffi_bindgen_react_native'}
    if names & forbidden:
        raise ValueError(f'composed shipping graph acquired unrelated dependencies: {sorted(names & forbidden)}')
    if any('/applications/' in packages[package]['manifest_path'] for package in selected):
        raise ValueError('reusable construction depends on application code')
    for package in selected:
        if packages[package]['name'] == 'prns-host-runtime':
            if ('connections' in nodes[package]['features']) != ('connections' in features):
                raise ValueError('saved connection owner selection differs from requested features')
        if packages[package]['name'] == 'prns-groups':
            if ('text' in nodes[package]['features']) != ('text' in features):
                raise ValueError('text profile selection differs from requested features')
    if ('prns-contact-card' in names) != ('contact-card' in features):
        raise ValueError('card codec selection differs from requested features')


def check_independent_connections(env):
    """Reuse the host owner's shipping-closure assertion without an aggregate."""
    spec = importlib.util.spec_from_file_location(
        'host_uniffi_check', ROOT / 'tools/repo/check-host-uniffi.py')
    checker = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(checker)
    checker.check_dependencies(env, connections=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--connections', action='store_true', help='qualify optional saved profiles and composed ownership')
    args = parser.parse_args()
    if sys.platform != 'darwin':
        raise SystemExit('composed Swift conformance requires macOS and Xcode command-line tools')
    env = tooling.environment(ROOT / 'target')
    target = Path(env['CARGO_TARGET_DIR'])
    if not target.is_absolute():
        target = ROOT / target
    env['CARGO_TARGET_DIR'] = str(target)
    target.mkdir(parents=True, exist_ok=True)
    features = ('archive', 'text', 'contact-card') + (('connections',) if args.connections else ())
    check_dependencies(env, ())
    check_dependencies(env, features)
    if args.connections:
        check_independent_connections(env)
    with tempfile.TemporaryDirectory(prefix='composed-swift-', dir=target) as temporary:
        output = Path(temporary)
        image = BINDINGS / 'image'
        recipe = tooling.BindingRecipe(
            manifest=image / 'Cargo.toml', package='prns-service-runtime-uniffi-image',
            library_name='prns_service_runtime_image', crate_dir=image, swift_dir=output,
            features=('uniffi-bindgen', *features))
        files = tooling.generated_files(recipe, None, env)
        for path, content in files.items():
            path.write_text(content)
        check_inventory('\n'.join(path.read_text() for path in output.glob('*.h')), args.connections)
        generated_sources = sorted(output.glob('*.swift'))
        fixture = output / 'consumer.swift'
        name = 'connections_conformance.swift' if args.connections else 'swift_conformance.swift'
        support = (BINDINGS / 'tests/swift_support.swift').read_text()
        if args.connections:
            support += (ROOT / 'prns-host/bindings/uniffi/tests/connections_journey.swift').read_text()
        fixture.write_text(support + (BINDINGS / 'tests' / name).read_text())
        executable = tooling.compile_swift_consumer(
            generated_sources=generated_sources,
            module_maps=sorted(output.glob('*.modulemap')), module_name='PrnsServiceRuntimeImage',
            native_library=target / 'debug/libprns_service_runtime_image.dylib',
            fixture=fixture, output_dir=output, env=env)
        subprocess.run([str(executable)], env=env, check=True, timeout=90)


if __name__ == '__main__':
    main()
