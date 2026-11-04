#!/usr/bin/env python3
"""Verify the selected standalone foreign dependency closure and Rust adapters."""
import json
from pathlib import Path
import subprocess

MANIFEST = Path(__file__).resolve().parents[1] / 'Cargo.toml'


def main():
    metadata = json.loads(subprocess.check_output([
        'cargo', 'metadata', '--locked', '--format-version', '1',
        '--manifest-path', str(MANIFEST),
    ], text=True))
    packages = {package['id']: package['name'] for package in metadata['packages']}
    nodes = {packages[node['id']]: node for node in metadata['resolve']['nodes']}
    forbidden = {'prns-group-text', 'prns-host-runtime', 'prns-host-snapshot', 'prns-runtime-tokio', 'prns-interfaces-tokio',
                 'personal-rns', 'prns-react-native', 'prns-lxmf', 'prns-app'}
    if found := forbidden & nodes.keys():
        raise SystemExit(f'standalone group bindings acquired unrelated dependencies: {sorted(found)}')
    if set(nodes['tokio']['features']) != {'sync'}:
        raise SystemExit('standalone bindings must not activate a Tokio executor')
    identity = set(nodes['prns-identities']['features'])
    if identity & {'owner', 'redb-store', 'fault-injection'}:
        raise SystemExit(f'key factory acquired identity service features: {sorted(identity)}')
    native = set(nodes['prns-groups']['features'])
    if 'uniffi-bindings' not in native or native & {'network', 'text', 'headless'}:
        raise SystemExit(f'wrong standalone group profile: {sorted(native)}')
    subprocess.run(['cargo', 'test', '--locked', '--manifest-path', str(MANIFEST),
                    '--workspace', '--all-targets'], check=True)


if __name__ == '__main__':
    main()
