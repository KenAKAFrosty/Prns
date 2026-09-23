#!/usr/bin/env python3
"""Build the native image and qualify actual generated foreign bindings."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
IMAGE = ROOT / 'prns-host/bindings/uniffi/image'


def run(*args):
    subprocess.run([str(arg) for arg in args], cwd=ROOT, check=True)


def main():
    run(sys.executable, ROOT / 'tools/repo/generate-host-contract.py', '--check')
    run('cargo', 'test', '--manifest-path', IMAGE.parent / 'Cargo.toml', '--test', 'transport')
    run('cargo', 'build', '--manifest-path', IMAGE / 'Cargo.toml',
        '--features', 'uniffi-bindgen', '--lib', '--bin', 'uniffi-bindgen')
    extension = '.dylib' if sys.platform == 'darwin' else '.dll' if sys.platform == 'win32' else '.so'
    library = IMAGE / 'target/debug' / (('' if sys.platform == 'win32' else 'lib') + 'prns_host_mobile' + extension)
    bindgen = IMAGE / 'target/debug' / ('uniffi-bindgen.exe' if sys.platform == 'win32' else 'uniffi-bindgen')
    with tempfile.TemporaryDirectory(prefix='foreign-conformance-', dir=IMAGE / 'target') as generated:
        output = Path(generated)
        run(bindgen, 'generate', '--library', library, '--language', 'python', '--out-dir', output, '--no-format')
        if sys.platform == 'win32': shutil.copy2(library, output / library.name)
        else: (output / library.name).symlink_to(library)
        run(sys.executable, IMAGE.parent / 'tests/foreign_conformance.py', output)
        run(sys.executable, IMAGE.parent / 'tests/remote_control_foreign_conformance.py', output)


if __name__ == '__main__':
    main()
