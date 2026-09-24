#!/usr/bin/env python3
"""Exercise app/SDK foreign object sharing in their one selected native image."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

APPLICATIONS = Path(__file__).resolve().parents[2]
ROOT = APPLICATIONS.parent
sys.path.insert(0, str(ROOT / 'tools/uniffi'))
import tooling


def main():
    env = tooling.environment(APPLICATIONS / 'target')
    target = Path(env['CARGO_TARGET_DIR'])
    def run(*args):
        subprocess.run([str(arg) for arg in args], cwd=APPLICATIONS, env=env, check=True)
    run('cargo', 'build', '--locked', '--manifest-path', APPLICATIONS / 'Cargo.toml',
        '-p', 'prns-app-native', '--features', 'host-test,uniffi-bindgen',
        '--lib', '--bin', 'uniffi-bindgen')
    extension = '.dylib' if sys.platform == 'darwin' else '.dll' if sys.platform == 'win32' else '.so'
    library = target / 'debug' / (('' if sys.platform == 'win32' else 'lib') + 'prns_app' + extension)
    bindgen = target / 'debug' / ('uniffi-bindgen.exe' if sys.platform == 'win32' else 'uniffi-bindgen')
    with tempfile.TemporaryDirectory(prefix='aggregate_foreign_', dir=target) as directory:
        generated = Path(directory)
        run(bindgen, 'generate', '--library', library, '--language', 'python',
            '--out-dir', generated, '--no-format')
        if sys.platform == 'win32':
            shutil.copy2(library, generated / library.name)
        else:
            (generated / library.name).symlink_to(library)
        run(sys.executable, APPLICATIONS / 'prns/native-composition/bindings/tests/foreign_aggregate.py', generated)


if __name__ == '__main__':
    main()
