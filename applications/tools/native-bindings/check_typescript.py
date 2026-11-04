#!/usr/bin/env python3
"""Typecheck the real generated namespaces against the selected SDK converter."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile

import generate


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--node-modules', type=Path,
                        default=generate.APPLICATIONS / 'node_modules',
                        help='existing installed dependencies; no packages are installed or changed')
    args = parser.parse_args()
    modules = args.node_modules.resolve()
    compiler = modules / 'typescript/bin/tsc'
    if not compiler.is_file():
        parser.error(f'TypeScript is not installed in {modules}')
    # Explicit paths also support inspecting this workspace against an existing
    # dependency installation without modifying either installation or sources.
    paths = {
        '@ubjs/core': [str(modules / '@ubjs/core')],
        '@ubjs/react-native': [str(modules / '@ubjs/react-native')],
        'personal-rns-expo/bindings': [str(generate.SDK_PACKAGE / 'src/bindings.native.ts')],
        'personal-rns-expo/bindings/values': [str(generate.SDK_PACKAGE / 'src/generated/prns_host_uniffi.ts')],
        'personal-rns/remote-control': [str(modules / 'personal-rns/dist/remote-control.generated.d.ts')],
        '*': [str(modules / '*')],
    }
    config = {
        'compilerOptions': {
            'target': 'ES2022', 'module': 'ESNext', 'moduleResolution': 'Bundler',
            'strict': True, 'noEmit': True, 'noImplicitOverride': True,
            'noUncheckedIndexedAccess': True, 'exactOptionalPropertyTypes': True,
            'verbatimModuleSyntax': True, 'skipLibCheck': True,
            'allowImportingTsExtensions': True, 'types': [],
            'lib': ['ES2022', 'DOM'], 'paths': paths,
        },
        'include': [str(generate.BINDINGS / '**/*.ts')],
    }
    with tempfile.TemporaryDirectory(prefix='prns-app-typescript-') as temporary:
        path = Path(temporary) / 'tsconfig.json'
        path.write_text(json.dumps(config, indent=2) + '\n')
        subprocess.run(['node', str(compiler), '--project', str(path)], check=True)


if __name__ == '__main__':
    main()
