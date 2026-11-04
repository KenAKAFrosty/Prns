#!/usr/bin/env python3
"""Run an ordinary Swift group consumer against one independent native image."""
from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[4]
BINDINGS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tools/uniffi'))
import tooling


def check_inventory(headers):
    # Reviewed exposure, not a production wrapper or a stable symbol ABI. The
    # generator discovers actual owning declarations; this fails if internal
    # scheduling, networking, raw keys, or a second domain facade leaks out.
    # This local image excludes the optional text and networking APIs.
    expected = {
        'prns_groups': ('func_create_request_id', 'func_default_group_profile',
            'func_resolve_group_profile', 'method_groupobservation_next', *(
            'method_client_' + name for name in (
                'identity', 'invitation', 'invitations', 'conversations', 'conversation', 'members', 'group_scope', 'execute', 'records',
                'record', 'delivery_progress', 'delivery_targets', 'changes', 'subscribe', 'submit', 'operation',
                'operations', 'dismiss_operation'))),
        'prns_groups_uniffi': (
            'constructor_grouplocalowner_create_new', 'constructor_grouplocalowner_open_existing',
            'method_grouplocalowner_client', 'method_grouplocalowner_shutdown', 'method_groupclose_wait'),
        'prns_identities_uniffi': (
            'constructor_keyowner_create_new', 'constructor_keyowner_open_existing',
            'constructor_keyowner_recover_creation', 'method_keyowner_identity_hash',
            'method_keyowner_lease', 'method_keylease_identity_hash'),
    }
    expected = {f'uniffi_{namespace}_fn_{name}' for namespace, names in expected.items() for name in names}
    actual = set(re.findall(r'\b(uniffi_\w+_fn_(?:constructor|method|func)_\w+)\s*\(', '\n'.join(headers)))
    if actual != expected:
        raise ValueError(f'group image exposure changed: missing={sorted(expected-actual)}, extra={sorted(actual-expected)}')


def main():
    if sys.platform != 'darwin':
        raise SystemExit('group Swift conformance requires macOS and Xcode command-line tools')
    env = tooling.environment(ROOT / 'target')
    target = Path(env['CARGO_TARGET_DIR'])
    if not target.is_absolute():
        target = ROOT / target
    env['CARGO_TARGET_DIR'] = str(target)
    target.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='group-swift-', dir=target) as temporary:
        output = Path(temporary)
        image = BINDINGS / 'image'
        recipe = tooling.BindingRecipe(
            manifest=image / 'Cargo.toml', package='prns-groups-uniffi-image',
            library_name='prns_group_services', crate_dir=image,
            swift_dir=output, features=('uniffi-bindgen',),
        )
        files = tooling.generated_files(recipe, None, env)
        for path, content in files.items():
            path.write_text(content)
        check_inventory(path.read_text() for path in output.glob('*.h'))
        executable = tooling.compile_swift_consumer(
            generated_sources=sorted(output.glob('*.swift')),
            module_maps=sorted(output.glob('*.modulemap')), module_name='PrnsGroupServices',
            native_library=target / 'debug/libprns_group_services.dylib',
            fixture=BINDINGS / 'tests/swift_conformance.swift', output_dir=output, env=env,
        )
        subprocess.run([str(executable)], env=env, check=True, timeout=90)


if __name__ == '__main__':
    main()
