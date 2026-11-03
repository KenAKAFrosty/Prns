#!/usr/bin/env python3
"""Qualify independent identity ownership and optional cards through generated Swift."""
import argparse
import json
import platform
from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[4]
BINDINGS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tools/uniffi'))
import tooling


def check_inventory(header, catalog=False, contact_card=False):
    # These entrypoints come from the real image metadata, through UniFFI's
    # generated C header. This is an exposure check, not a promised stable ABI.
    prefix = 'uniffi_prns_identities_uniffi_fn_'
    expected = {prefix + name for name in (
        'constructor_keyowner_create_new', 'constructor_keyowner_open_existing',
        'constructor_keyowner_recover_creation', 'method_keyowner_identity_hash',
        'method_keyowner_lease', 'method_keylease_identity_hash',
    )}
    if contact_card:
        expected.add(prefix + 'method_keylease_contact_card')
        expected.update('uniffi_prns_contact_card_fn_func_' + name
                        for name in ('verify_contact_card', 'service_name_hash',
                                     'encode_contact_card_uri', 'decode_contact_card_uri'))
    if catalog:
        expected.update(prefix + name for name in (
            'constructor_identitycatalogowner_create_new',
            'constructor_identitycatalogowner_open_existing',
            'method_identitycatalogowner_client', 'method_identitycatalogowner_shutdown',
            'method_identitycatalogclose_wait', 'method_keylease_import_key'))
        expected.update('uniffi_prns_identities_fn_method_identitycatalogclient_' + name
                        for name in ('identities', 'operations', 'find_reserved', 'cancel_reserved',
                                     'prune_completed', 'creation_scope', 'create_or_recover',
                                     'retire_creations', 'reserve', 'operation', 'identity'))
        expected.update('uniffi_prns_identities_fn_method_identityclient_' + name
                        for name in ('reference', 'snapshot', 'prepare', 'begin', 'apply_stop',
                                     'recover', 'update_metadata'))
        expected.update('uniffi_prns_identities_fn_method_identityannounceclient_' + name
                        for name in ('settings', 'set_settings', 'announce'))
        if contact_card:
            expected.add('uniffi_prns_identities_fn_method_identityclient_contact_card')
    actual = set(re.findall(r'\b(uniffi_\w+_fn_(?:constructor|method|func)_\w+)\s*\(', header))
    if actual != expected:
        raise ValueError(f'identity metadata exposure changed: missing={sorted(expected-actual)}, extra={sorted(actual-expected)}')


def check_dependencies(env, catalog, contact_card=False):
    target = 'aarch64-apple-darwin' if platform.machine() == 'arm64' else 'x86_64-apple-darwin'
    command = ['cargo', 'metadata', '--locked', '--format-version', '1', '--manifest-path',
               BINDINGS / 'image/Cargo.toml', '--filter-platform', target]
    features = (['catalog'] if catalog else []) + (['contact-card'] if contact_card else [])
    if features:
        command.extend(['--features', ','.join(features)])
    metadata = json.loads(tooling.run(*command, env=env, capture=True))
    packages = {package['id']: package for package in metadata['packages']}
    nodes = {node['id']: node for node in metadata['resolve']['nodes']}
    pending = [next(key for key, value in packages.items()
                    if value['name'] == 'prns-identities-uniffi-image')]
    selected = set()
    while pending:
        package = pending.pop()
        if package not in selected:
            selected.add(package)
            pending.extend(dep['pkg'] for dep in nodes[package]['deps'])
    names = {packages[package]['name'] for package in selected}
    forbidden = {'redb', 'prns-service-runtime', 'prns-host', 'prns-host-runtime', 'prns-groups-core', 'prns-groups',
                 'prns-lxmf', 'prns-lxmf-wire'}
    if not catalog:
        forbidden.add('tokio')
    unexpected = names & forbidden
    if unexpected:
        raise ValueError(f'independent identity image acquired unrelated dependencies: {sorted(unexpected)}')
    native_features = next(nodes[package]['features'] for package in selected
                           if packages[package]['name'] == 'prns-identities')
    if not catalog and {'owner', 'uniffi-bindings'} & set(native_features):
        raise ValueError(f'key-only image acquired the catalog owner: {native_features}')
    if ('prns-contact-card' in names) != contact_card:
        raise ValueError('contact-card codec selection differs from requested profile')
    if 'tokio' in names:
        features = next(nodes[package]['features'] for package in selected
                        if packages[package]['name'] == 'tokio')
        if set(features) != {'sync'}:
            raise ValueError(f'identity image acquired an ambient executor: {features}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--catalog', action='store_true', help='qualify the optional canonical catalog owner')
    parser.add_argument('--contact-card', action='store_true', help='qualify optional service-neutral contact-card issuance')
    args = parser.parse_args()
    if sys.platform != 'darwin':
        raise SystemExit('key Swift conformance requires macOS and Xcode command-line tools')
    env = tooling.environment(ROOT / 'target')
    target = Path(env['CARGO_TARGET_DIR'])
    if not target.is_absolute():
        target = ROOT / target
    env['CARGO_TARGET_DIR'] = str(target)
    target.mkdir(parents=True, exist_ok=True)
    if args.contact_card:
        check_dependencies(env, args.catalog, contact_card=True)
    else:
        check_dependencies(env, args.catalog)
    with tempfile.TemporaryDirectory(prefix='identity-key-swift-', dir=target) as temporary:
        output = Path(temporary)
        image = BINDINGS / 'image'
        recipe = tooling.BindingRecipe(
            manifest=image / 'Cargo.toml', package='prns-identities-uniffi-image',
            library_name='prns_identity_keys', crate_dir=image,
            swift_dir=output, features=tuple(['uniffi-bindgen']
                + (['catalog'] if args.catalog else [])
                + (['contact-card'] if args.contact_card else [])),
        )
        files = tooling.generated_files(recipe, None, env)
        for path, content in files.items():
            path.write_text(content)
        check_inventory('\n'.join(path.read_text() for path in output.glob('*.h')), args.catalog, args.contact_card)
        generated_sources = sorted(output.glob('*.swift'))
        fixture = BINDINGS / ('tests/catalog_conformance.swift' if args.catalog else 'tests/swift_conformance.swift')
        if args.contact_card:
            # Shared, bounded wire verification belongs to these test consumers;
            # generated source remains exactly the owning UniFFI declarations.
            fixture = output / 'contact_card_consumer.swift'
            body = BINDINGS / ('tests/catalog_card_conformance.swift' if args.catalog else 'tests/key_card_conformance.swift')
            fixture.write_text((BINDINGS / 'tests/contact_card_checks.swift').read_text() + '\n' + body.read_text())
        executable = tooling.compile_swift_consumer(
            generated_sources=generated_sources,
            module_maps=sorted(output.glob('*.modulemap')),
            module_name='PrnsIdentityCatalog' if args.catalog else 'PrnsIdentityKeys',
            native_library=target / 'debug/libprns_identity_keys.dylib',
            fixture=fixture,
            output_dir=output, env=env,
        )
        subprocess.run([str(executable)], env=env, check=True, timeout=60)


if __name__ == '__main__':
    main()
