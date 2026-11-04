#!/usr/bin/env python3
"""Shared pinned UniFFI generation, native builds, and owned-output checks.

Callers supply their metadata crate and output locations. This module has no
application-specific imports, feature flags, names, or generated contracts.
"""
import importlib.util
import json
import os
import re
from pathlib import Path
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from functools import cache

HERE = Path(__file__).resolve().parent
REACT_NATIVE_TOOL = HERE / 'react_native.py'


@cache
def _react_native_module():
    # Native-only recipes need no React Native tooling or dependency metadata.
    # Load it only when a caller selects the React Native generator/build path.
    spec = importlib.util.spec_from_file_location('prns_react_native_tooling', REACT_NATIVE_TOOL)
    native = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(native)
    return native


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
    source, _ = _react_native_module().prepare(cache, lock, source_id=source_id)
    cli_env = generator_environment(cache, env)
    run('cargo', 'build', '--locked', '--no-default-features', '-p',
        'uniffi-bindgen-react-native', cwd=source, env=cli_env)
    return Path(cli_env['CARGO_TARGET_DIR']) / 'debug/uniffi-bindgen-react-native'


def generator_inputs():
    """Identify the upstream generator and its TypeScript emitter patches."""
    native = _react_native_module()
    lock = native.inputs()
    return lock, native.generator_id(lock)


def generator_environment(cache, env):
    # The generator is third-party source built as a root Cargo package, so
    # Cargo's dependency lint cap does not apply. Keep consumer compiler policy on
    # the consumer, without changing the pinned upstream package to silence warnings.
    target = Path(env.get('PRNS_UBRN_TARGET_DIR') or cache / 'cli-target').expanduser().resolve()
    cli_env = dict(env, CARGO_TARGET_DIR=str(target))
    for key in tuple(cli_env):
        if key in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS') or (
                key.startswith('CARGO_') and key.endswith('_RUSTFLAGS')):
            del cli_env[key]
    return cli_env


def normalize(text):
    return '\n'.join(line.rstrip() for line in text.splitlines()).rstrip() + '\n'


# UniFFI 0.31.2 intentionally uses an Int JNA return carrier for Rust u16.
# Its checksum template omitted the narrowing performed by ordinary UShort
# converters. Upstream #2935 (released in 0.32.1) fixes that exact comparison.
# Keep this backport at the shared generation boundary, with no Cargo-cache or
# generated-output edits. A version/template change requires explicit review.
_KOTLIN_BINDGEN_VERSION = 'uniffi-bindgen 0.31.2'
_KOTLIN_CHECKSUM_FUNCTION = re.compile(
    r'(?P<header>^private fun uniffiCheckApiChecksums\(lib: IntegrityCheckingUniffiLib\) \{\n)'
    r'(?P<body>.*?)(?P<footer>^\})', re.MULTILINE | re.DOTALL)
_KOTLIN_CHECKSUM_CHECK = re.compile(
    r'    if \(lib\.(?P<name>uniffi_\w+_checksum_\w+)\(\) != (?P<expected>[0-9]+)\) \{\n'
    r'        throw RuntimeException\("UniFFI API checksum mismatch: try cleaning and rebuilding your project"\)\n'
    r'    \}\n')
_KOTLIN_CHECKSUM_DECLARATION = re.compile(
    r'^    external fun (uniffi_\w+_checksum_\w+)\(\n    \): Int$', re.MULTILINE)


def normalize_kotlin_checksums(source, version):
    """Backport only the reviewed 0.31.2 checksum comparison; reject drift.

    UShort.toInt() compares the actual Rust u16, irrespective of the Int
    carrier's unspecified upper bits. Keep every expected checksum, throw,
    native declaration and contract-version check byte-for-byte unchanged.
    """
    if version != _KOTLIN_BINDGEN_VERSION:
        raise ValueError(f'Kotlin checksum normalization needs reviewed bindgen version: {version!r}')
    functions = list(_KOTLIN_CHECKSUM_FUNCTION.finditer(source))
    if len(functions) != 1:
        raise ValueError('Kotlin checksum function missing or duplicated')
    function = functions[0]
    body = function['body']
    checks = list(_KOTLIN_CHECKSUM_CHECK.finditer(body))
    declarations = _KOTLIN_CHECKSUM_DECLARATION.findall(source)
    symbols = re.findall(r'\bexternal fun (uniffi_\w+_checksum_\w+)\(', source)
    names = [check['name'] for check in checks]
    if (''.join(check[0] for check in checks) != body
            or len(set(names)) != len(names)
            or len(set(declarations)) != len(declarations)
            or set(names) != set(declarations)
            or declarations != symbols
            or any(int(check['expected']) > 0xffff for check in checks)):
        raise ValueError('unreviewed Kotlin checksum template or Int declarations')
    corrected = _KOTLIN_CHECKSUM_CHECK.sub(
        lambda check: check[0].replace(
            f"lib.{check['name']}()", f"lib.{check['name']}().toUShort().toInt()", 1), body)
    return source[:function.start('body')] + corrected + source[function.end('body'):]


@dataclass(frozen=True)
class BindingRecipe:
    """Metadata and output locations for one selected native image."""
    manifest: Path
    package: str
    library_name: str
    crate_dir: Path
    # A missing destination omits that language and its generator entirely.
    typescript_dir: Path | None = None
    swift_dir: Path | None = None
    kotlin_dir: Path | None = None
    uniffi_config: Path | None = None
    features: tuple[str, ...] = ()
    extra_bins: tuple[str, ...] = ()
    bindgen_binary: str = 'uniffi-bindgen'


def generated_files(recipe, cli, env):
    """Generate selected languages; native-only recipes pass ``cli=None``.

    The RN generator is required only for TypeScript. A Swift/Kotlin consumer
    builds the image's ordinary UniFFI CLI and needs no ubrn checkout or Node.
    """
    languages = [language for language, destination in (
        ('swift', recipe.swift_dir), ('kotlin', recipe.kotlin_dir))
        if destination is not None]
    if recipe.typescript_dir is None and not languages:
        raise ValueError('binding recipe must select at least one output language')
    if recipe.typescript_dir is not None and cli is None:
        raise ValueError('TypeScript output requires the pinned RN generator')
    command = ['cargo', 'build', '--locked', '--manifest-path', recipe.manifest,
               '-p', recipe.package, '--lib']
    binaries = [*([recipe.bindgen_binary] if languages else []), *recipe.extra_bins]
    for binary in dict.fromkeys(binaries):
        command.extend(['--bin', binary])
    if recipe.features:
        command.extend(['--features', ','.join(recipe.features)])
    run(*command, cwd=recipe.manifest.parent, env=env)
    target = Path(env['CARGO_TARGET_DIR']).resolve()
    if sys.platform == 'win32':
        library = target / f'debug/{recipe.library_name}.dll'
        bindgen = target / 'debug' / f'{recipe.bindgen_binary}.exe'
    else:
        suffix = 'dylib' if sys.platform == 'darwin' else 'so'
        library = target / f'debug/lib{recipe.library_name}.{suffix}'
        bindgen = target / 'debug' / recipe.bindgen_binary
    with tempfile.TemporaryDirectory(prefix='prns-generated-', dir=target) as temporary:
        temp = Path(temporary)
        files = {}
        if recipe.typescript_dir is not None:
            ts = temp / 'typescript'
            run(cli, 'generate', 'jsi2', 'bindings', '--library', library,
                '--ts-dir', ts, '--lib-name', recipe.library_name, '--no-format',
                cwd=recipe.crate_dir, env=env)
            files.update({recipe.typescript_dir / file.name: normalize(file.read_text())
                          for file in ts.glob('*.ts')})
        if languages:
            kotlin_version = None
            if recipe.kotlin_dir is not None:
                kotlin_version = run(bindgen, '--version', cwd=recipe.manifest.parent,
                                     env=env, capture=True).strip()
                if kotlin_version != _KOTLIN_BINDGEN_VERSION:
                    raise ValueError(f'Kotlin generation requires reviewed {_KOTLIN_BINDGEN_VERSION}; '
                                     f'got {kotlin_version!r}')
            native = temp / 'native'
            config_args = ('--config', recipe.uniffi_config) if recipe.uniffi_config else ()
            language_args = [arg for language in languages for arg in ('--language', language)]
            run(bindgen, 'generate', '--library', library,
                *config_args, *language_args, '--no-format', '--out-dir', native,
                cwd=recipe.manifest.parent, env=env)
            if recipe.swift_dir is not None:
                for file in native.iterdir():
                    if file.is_file() and file.suffix in ('.swift', '.h', '.modulemap'):
                        files[recipe.swift_dir / file.name] = normalize(file.read_text())
            if recipe.kotlin_dir is not None:
                for file in native.rglob('*.kt'):
                    files[recipe.kotlin_dir / file.relative_to(native)] = normalize_kotlin_checksums(
                        normalize(file.read_text()), kotlin_version)
        return files


def compile_swift_consumer(*, generated_sources, module_maps, module_name,
                           native_library, fixture, output_dir, env):
    """Compile image namespaces into one Swift module and a separate consumer.

    Cross-namespace generated types share a module. All image, module and source
    choices belong to the caller; this helper neither generates nor executes.
    """
    generated_sources, module_maps = tuple(generated_sources), tuple(module_maps)
    if not generated_sources or not module_maps:
        raise ValueError('Swift compilation requires generated sources and module maps')
    module_library = output_dir / f'lib{module_name}.dylib'
    # Keep compiler outputs in the selected build tree. Swift otherwise writes
    # its implicit module cache under the user's home, outside this recipe's
    # output ownership and writable paths. Both compilation steps share it.
    module_cache = Path(env.get('CARGO_TARGET_DIR', output_dir)).expanduser().resolve() / 'swift-module-cache'
    module_args = ['-module-cache-path', module_cache]
    for parent in dict.fromkeys(path.parent for path in module_maps):
        module_args.extend(('-I', parent))
    for module_map in module_maps:
        module_args.extend(('-Xcc', f'-fmodule-map-file={module_map}'))
    run('xcrun', 'swiftc', '-parse-as-library', '-emit-library', '-emit-module',
        '-module-name', module_name, '-emit-module-path', output_dir / f'{module_name}.swiftmodule',
        *module_args, native_library, '-Xlinker', '-rpath', '-Xlinker', native_library.parent,
        *generated_sources, '-o', module_library, cwd=output_dir, env=env)
    executable = output_dir / 'swift-conformance'
    run('xcrun', 'swiftc', '-parse-as-library', '-I', output_dir, *module_args,
        module_library, '-Xlinker', '-rpath', '-Xlinker', output_dir,
        fixture, '-o', executable, cwd=output_dir, env=env)
    return executable


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
        expected = _react_native_module().inputs()['toolchain']['androidNdk']
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
