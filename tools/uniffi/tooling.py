#!/usr/bin/env python3
"""Shared pinned UniFFI generation, native builds, and owned-output checks.

Callers supply their metadata crate and output locations. This module has no
application-specific imports, feature flags, names, or generated contracts.
"""
import importlib.util
import hashlib
import json
import os
import re
from pathlib import Path
import subprocess
import sys
import tempfile
from dataclasses import dataclass

HERE = Path(__file__).resolve().parent
VENDOR_TOOL = HERE.parent / 'ubrn-vendor/vendor.py'
spec = importlib.util.spec_from_file_location('prns_ubrn_vendor', VENDOR_TOOL)
vendor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vendor)


def run(*args, cwd=None, env=None, capture=False):
    result = subprocess.run([str(arg) for arg in args], cwd=cwd, env=env, check=True,
                            stdout=subprocess.PIPE if capture else None, text=True)
    return result.stdout if capture else None


def environment(target_dir):
    env = dict(os.environ)
    env.setdefault('CARGO_TARGET_DIR', str(target_dir))
    env.setdefault('CARGO_BUILD_JOBS', '4')
    env.setdefault('RUSTUP_TOOLCHAIN', 'stable')
    # RUSTUP_TOOLCHAIN only affects rustup shims. A system Cargo earlier on
    # PATH would otherwise compile with a different Rust than the selected one.
    rustc = run('rustup', 'which', '--toolchain', env['RUSTUP_TOOLCHAIN'], 'rustc',
                env=env, capture=True).strip()
    env['PATH'] = str(Path(rustc).parent) + os.pathsep + env['PATH']
    if sys.platform == 'darwin':
        clang = run('xcrun', '--find', 'clang', capture=True).strip()
        env.update(PATH=str(Path(clang).parent) + os.pathsep + env['PATH'], CC=clang,
                   CXX=run('xcrun', '--find', 'clang++', capture=True).strip(),
                   AR=run('xcrun', '--find', 'ar', capture=True).strip(),
                   RANLIB=run('xcrun', '--find', 'ranlib', capture=True).strip(), LIBRARY_PATH='')
        for key in ('SDKROOT', 'CFLAGS', 'CXXFLAGS', 'CPPFLAGS'):
            env.pop(key, None)
    return env


def generator(cache, env):
    lock, source_id = generator_inputs()
    source, _ = vendor.prepare(cache, lock, source_id=source_id)
    cli_env = generator_environment(cache, env)
    run('cargo', 'build', '--locked', '--no-default-features', '-p',
        'uniffi-bindgen-react-native', cwd=source, env=cli_env)
    return cache / 'cli-target/debug/uniffi-bindgen-react-native'


def generator_inputs():
    """One generator source, extending the pinned runtime tree only in templates.

    Keeping these patches separate preserves the provenance of native runtime
    archives: a TypeScript emitter correction does not claim they were rebuilt.
    """
    lock = vendor.inputs()
    manifest = vendor.VENDOR / 'generator-patches.json'
    extension = json.loads(manifest.read_text())
    if extension.get('schemaVersion') != 1:
        raise ValueError('unsupported generator patch version')
    for patch in extension['patches']:
        path = vendor.VENDOR / patch['file']
        if path.resolve().parent != (vendor.VENDOR / 'patches').resolve():
            raise ValueError('generator patch must belong to the reviewed patches directory')
        if vendor.digest(path) != patch['sha256']:
            raise ValueError(f'generator patch hash mismatch: {path.name}')
        destinations = re.findall(r'^\+\+\+ b/(.+)$', path.read_text(), re.MULTILINE)
        if not destinations or any(not name.startswith(
                'crates/ubrn_bindgen/src/bindings/gen_typescript/') for name in destinations):
            raise ValueError('generator-only patches may change TypeScript emitter sources only')
    source_id = hashlib.sha256((vendor.digest(vendor.LOCK) + vendor.digest(manifest)).encode()).hexdigest()
    lock['patches'] = [*lock['patches'], *extension['patches']]
    return lock, source_id


def generator_environment(cache, env):
    # The generator is third-party source built as a root Cargo package, so
    # Cargo's dependency lint cap does not apply. Keep consumer compiler policy on
    # the consumer, without changing the pinned upstream package to silence warnings.
    cli_env = dict(env, CARGO_TARGET_DIR=str(cache / 'cli-target'))
    for key in tuple(cli_env):
        if key in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS') or (
                key.startswith('CARGO_') and key.endswith('_RUSTFLAGS')):
            del cli_env[key]
    return cli_env


def normalize(text):
    return '\n'.join(line.rstrip() for line in text.splitlines()).rstrip() + '\n'


@dataclass(frozen=True)
class BindingRecipe:
    """Metadata and output locations for one selected native image."""
    manifest: Path
    package: str
    library_name: str
    crate_dir: Path
    typescript_dir: Path
    swift_dir: Path
    kotlin_dir: Path
    uniffi_config: Path | None = None
    features: tuple[str, ...] = ()
    extra_bins: tuple[str, ...] = ()
    bindgen_binary: str = 'uniffi-bindgen'


def generated_files(recipe, cli, env):
    command = ['cargo', 'build', '--locked', '--manifest-path', recipe.manifest,
               '-p', recipe.package, '--lib', '--bin', recipe.bindgen_binary]
    for binary in recipe.extra_bins:
        command.extend(['--bin', binary])
    if recipe.features:
        command.extend(['--features', ','.join(recipe.features)])
    run(*command, cwd=recipe.manifest.parent, env=env)
    target = Path(env['CARGO_TARGET_DIR']).resolve()
    suffix = 'dylib' if sys.platform == 'darwin' else 'so'
    library = target / f'debug/lib{recipe.library_name}.{suffix}'
    with tempfile.TemporaryDirectory(prefix='prns-generated-', dir=target) as temporary:
        temp = Path(temporary)
        ts, native = temp / 'typescript', temp / 'native'
        run(cli, 'generate', 'jsi2', 'bindings', '--library', library,
            '--ts-dir', ts, '--lib-name', recipe.library_name, '--no-format',
            cwd=recipe.crate_dir, env=env)
        config_args = ('--config', recipe.uniffi_config) if recipe.uniffi_config else ()
        run(target / 'debug' / recipe.bindgen_binary, 'generate', '--library', library,
            *config_args, '--language', 'swift', '--language', 'kotlin',
            '--no-format', '--out-dir', native, cwd=recipe.manifest.parent, env=env)
        files = {recipe.typescript_dir / file.name: normalize(file.read_text())
                 for file in ts.glob('*.ts')}
        for file in native.iterdir():
            if file.is_file():
                files[recipe.swift_dir / file.name] = normalize(file.read_text())
        for file in native.rglob('*.kt'):
            files[recipe.kotlin_dir / file.relative_to(native)] = normalize(file.read_text())
        return files


def ownership(root, manifest):
    """Load reviewed output scopes, rejecting escapes and symlinked parents."""
    document = json.loads(manifest.read_text())
    if document.get('schemaVersion') != 1:
        raise ValueError('unsupported generated-output ownership version')
    root = root.resolve()

    def paths(key):
        result = []
        for value in document[key]:
            relative = Path(value)
            if relative.is_absolute() or not relative.parts or any(
                    part in ('', '.', '..') for part in value.split('/')):
                raise ValueError(f'invalid generated-output path: {value}')
            path = root / relative
            if any(part.is_symlink() for part in (path, *path.parents) if part != root):
                raise ValueError(f'symlink in generated-output path: {value}')
            if not path.resolve().is_relative_to(root):
                raise ValueError(f'generated-output path escapes ownership root: {value}')
            result.append(path)
        return result

    directories, singles = paths('directories'), paths('files')
    scaffolding = paths('scaffolding')
    if len(set(directories + singles + scaffolding)) != len(directories + singles + scaffolding):
        raise ValueError('generated-output ownership paths must be unique')
    if any(first != second and first.is_relative_to(second)
           for first in directories for second in directories):
        raise ValueError('generated-output directories must not overlap')
    if any(path.is_relative_to(directory) for path in singles + scaffolding for directory in directories):
        raise ValueError('individual files and scaffolding must be outside generated directories')
    return directories, singles


def synchronize_outputs(files, check, root, manifest):
    """Compare the complete owned output set; remove obsolete owned files only."""
    directories, singles = ownership(root, manifest)
    root = root.resolve()
    expected = set(files)
    for path in expected:
        if not path.is_absolute() or path != path.resolve():
            raise ValueError(f'generated output must be an absolute, non-symlink path: {path}')
        if path not in singles and not any(path.is_relative_to(directory) for directory in directories):
            raise ValueError(f'generator produced an unowned path: {path}')
        if any(part.is_symlink() for part in (path, *path.parents) if part != root):
            raise ValueError(f'symlink in generated output: {path}')
    if not set(singles).issubset(expected):
        raise ValueError('generator omitted an individually owned output')
    if any(not any(path.is_relative_to(directory) for path in expected) for directory in directories):
        raise ValueError('generator omitted an owned output directory')
    existing = existing_outputs(directories, singles)
    obsolete = existing - expected
    changed = {path for path, content in files.items()
               if not path.exists() or path.read_text() != content}
    if check and (changed or obsolete):
        details = [str(path.relative_to(root)) for path in sorted(changed)]
        details += [f'{path.relative_to(root)} (obsolete)' for path in sorted(obsolete)]
        raise ValueError('generated bindings differ; regenerate the selected binding recipe:\n' + '\n'.join(details))
    if not check:
        for path in sorted(obsolete):
            path.unlink()
        for path in sorted(changed):
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(files[path])


def existing_outputs(directories, singles):
    existing = {path for path in singles if path.exists()}
    for directory in directories:
        if directory.exists() and not directory.is_dir():
            raise ValueError(f'generated output directory is not a directory: {directory}')
        for path in directory.rglob('*'):
            if path.is_symlink():
                raise ValueError(f'symlink in generated output directory: {path}')
            if path.is_file():
                existing.add(path)
    return existing


def build_mobile(cli, env, args, config):
    if args.command == 'android':
        expected = vendor.inputs()['toolchain']['androidNdk']
        sdk = Path(env.get('ANDROID_HOME') or env.get('ANDROID_SDK_ROOT') or
                   (Path.home() / ('Library/Android/sdk' if sys.platform == 'darwin' else 'Android/Sdk')))
        ndk = Path(env.get('ANDROID_NDK_HOME') or sdk / 'ndk' / expected).expanduser().resolve()
        properties = dict(tuple(part.strip() for part in line.split('=', 1))
                          for line in (ndk / 'source.properties').read_text().splitlines() if '=' in line)
        if properties.get('Pkg.Revision') != expected:
            raise ValueError(f'Android bindings require NDK {expected}; set ANDROID_NDK_HOME to that installation')
        env = dict(env, ANDROID_NDK_HOME=str(ndk), NDK_HOME=str(ndk))
    command = [cli, 'build', 'jsi2', args.command, '--config', config]
    if args.release:
        command.append('--release')
    if args.targets:
        command.extend(['--targets', args.targets])
    if args.command == 'ios' and args.sim_only:
        command.append('--sim-only')
    run(*command, cwd=config.parent, env=env)
