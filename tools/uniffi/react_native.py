#!/usr/bin/env python3
"""Pinned upstream npm packages and cached React Native binding generation."""

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile
import urllib.request

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
LOCK = HERE / 'react-native.json'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def patch_paths(text):
    """Inspect both sides of ordinary text patches, including deletes/renames."""
    headers = [line for line in text.splitlines() if line.startswith('diff --git ')]
    matches = [re.fullmatch(r'diff --git a/(\S+) b/(\S+)', line) for line in headers]
    if (not headers or not all(matches) or 'GIT binary patch' in text
            or any(line.startswith('Binary files ') for line in text.splitlines())):
        raise ValueError('expected a Git text patch')
    paths = [path for match in matches for path in match.groups()]
    for line in text.splitlines():
        if line.startswith(('--- ', '+++ ')):
            if line in ('--- /dev/null', '+++ /dev/null'):
                continue
            match = re.fullmatch(r'(?:--- a/|\+\+\+ b/)(\S+)', line)
            if match is None:
                raise ValueError('expected Git text patch file headers')
            paths.append(match[1])
    if any(part in ('', '.', '..') for path in paths for part in path.split('/')):
        raise ValueError('invalid patch path')
    return tuple(paths)


def inputs():
    lock = json.loads(LOCK.read_text())
    if lock.get('schemaVersion') != 1:
        raise ValueError('unsupported React Native dependency configuration')
    for patch in lock['patches']:
        path = HERE / patch['file']
        if path.resolve().parent != (HERE / 'patches').resolve():
            raise ValueError('generator patch must belong to tools/uniffi/patches')
        if digest(path) != patch['sha256']:
            raise ValueError(f'generator patch digest mismatch: {path.name}')
        if any(not name.startswith(
                'crates/ubrn_bindgen/src/bindings/gen_typescript/')
                for name in patch_paths(path.read_text())):
            raise ValueError('generator patches may change TypeScript emitter sources only')
    runtime = lock['runtime_patch']
    path = ROOT / runtime['file']
    if not path.resolve().is_relative_to(ROOT.resolve()):
        raise ValueError('runtime patch must belong to the repository')
    if digest(path) != runtime['sha256']:
        raise ValueError('runtime patch digest mismatch')
    if any(not name.startswith('node_modules/@ubjs/react-native/')
           or Path(name).suffix not in ('.cpp', '.h', '.kt', '.mm', '.plist')
           for name in patch_paths(path.read_text())):
        raise ValueError('runtime patch may change only native source and framework metadata')
    for package in lock['packages']:
        path = HERE / package['license_file']
        if path.resolve().parent != (HERE / 'licenses').resolve():
            raise ValueError('package license must belong to tools/uniffi/licenses')
        if digest(path) != package['license_sha256']:
            raise ValueError(f"license digest mismatch: {package['name']}")
    return lock


def generator_id(lock):
    return hashlib.sha256(json.dumps(
        {'upstream': lock['upstream'], 'patches': lock['patches']},
        sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def run(*args, cwd=None, env=None, capture=False):
    result = subprocess.run([str(arg) for arg in args], cwd=cwd, env=env,
                            check=True, text=True,
                            stdout=subprocess.PIPE if capture else None)
    return result.stdout.strip() if capture else None


def prepare(cache, lock, source_id=None):
    """Fetch upstream into disposable storage; verify the patched tree on reuse."""
    source = Path(cache) / (source_id or generator_id(lock))[:16] / 'source'
    if not source.exists():
        source.mkdir(parents=True)
        run('git', 'init', '--quiet', source)
        run('git', 'fetch', '--depth=1', lock['upstream']['repository'],
            lock['upstream']['revision'], cwd=source)
        run('git', 'checkout', '--detach', 'FETCH_HEAD', cwd=source)
        for patch in lock['patches']:
            run('git', 'apply', '--index', HERE / patch['file'], cwd=source)
        run('git', '-c', 'user.name=PRNS binding generator', '-c',
            'user.email=generator@invalid', 'commit', '--quiet', '-m',
            'Apply pinned binding generator patches', cwd=source)
    if run('git', 'rev-parse', 'HEAD^', cwd=source, capture=True) != lock['upstream']['revision']:
        raise ValueError('cached generator is not based on the pinned upstream revision')
    run('git', 'diff', '--exit-code', 'HEAD', cwd=source)
    with tempfile.TemporaryDirectory(prefix='expected-', dir=source.parent) as directory:
        env = dict(os.environ, GIT_INDEX_FILE=str(Path(directory) / 'index'))
        run('git', 'read-tree', lock['upstream']['revision'], cwd=source, env=env)
        for patch in lock['patches']:
            run('git', 'apply', '--cached', HERE / patch['file'], cwd=source, env=env)
        expected = run('git', 'write-tree', cwd=source, env=env, capture=True)
    actual = run('git', 'rev-parse', 'HEAD^{tree}', cwd=source, capture=True)
    if actual != expected:
        raise ValueError('cached generator differs from the pinned patches')
    return source, actual


def cache_directory():
    return Path(os.environ.get('PRNS_UBRN_CACHE', ROOT / 'target/uniffi')).expanduser().resolve()


def package_archive(package, cache=None):
    """Read the exact published archive; cached bytes must retain npm integrity."""
    directory = Path(cache or cache_directory()) / 'npm'
    directory.mkdir(parents=True, exist_ok=True)
    archive = directory / (package['sha256'] + '.tgz')
    if not archive.exists():
        with urllib.request.urlopen(package['archive'], timeout=60) as response:
            contents = response.read()
        verify_archive_bytes(contents, package)
        with tempfile.NamedTemporaryFile(dir=directory, delete=False) as output:
            output.write(contents)
            temporary = Path(output.name)
        temporary.replace(archive)
    verify_archive_bytes(archive.read_bytes(), package)
    with tarfile.open(archive, 'r:gz') as source:
        try:
            member = source.getmember('package/package.json')
        except KeyError as error:
            raise ValueError('package archive lacks package.json') from error
        if not member.isfile():
            raise ValueError('package metadata must be a regular file')
        metadata = json.load(source.extractfile(member))
    if any(metadata.get(field) != package[field] for field in ('name', 'version', 'license')):
        raise ValueError(f"package identity mismatch: {package['name']}")
    return archive


def verify_archive_bytes(contents, package):
    integrity = 'sha512-' + base64.b64encode(hashlib.sha512(contents).digest()).decode()
    if hashlib.sha256(contents).hexdigest() != package['sha256'] or integrity != package['integrity']:
        raise ValueError(f"package integrity mismatch: {package['name']}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['check'])
    parser.parse_args()
    try:
        lock = inputs()
        for package in lock['packages']:
            package_archive(package)
    except (OSError, ValueError, tarfile.TarError) as error:
        parser.exit(1, f'React Native inputs: {error}\n')
    print('Pinned npm package integrity, licenses and patch inputs match')


if __name__ == '__main__':
    main()
