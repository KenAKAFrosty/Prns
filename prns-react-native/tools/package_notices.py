"""Generate attribution for one canonical SDK image and its verified runtime peers."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys

PACKAGE = Path(__file__).resolve().parents[1]
ROOT = PACKAGE.parent
sys.path.insert(0, str(PACKAGE / 'tools'))
import generate

SPEC = importlib.util.spec_from_file_location('sdk_notice_generator', ROOT / 'tools/repo/generate-third-party-notices.py')
notice_generator = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(notice_generator)
NOTICE = 'THIRD_PARTY_NOTICES.md'
SOURCES = 'third-party-sources.json'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def selected_inputs(provider_path, provider, runtime):
    """Cargo owns the selected dependency closure; no second package inventory."""
    paths = set(notice_generator.runtime_notices.input_paths())
    paths.update((Path(__file__).resolve(), Path(notice_generator.__file__).resolve(),
                  provider_path, notice_generator.ABOUT, PACKAGE / 'package.json',
                  ROOT / 'LICENSE-MIT', ROOT / 'LICENSE-APACHE'))
    packages = {}
    for target in runtime['source']['targets']:
        command = ['cargo', 'metadata', '--locked', '--format-version', '1',
                   '--manifest-path', str(provider['manifest']), '--filter-platform', target]
        if provider.get('features'):
            command.extend(['--features', ','.join(provider['features'])])
        result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, check=True)
        metadata = json.loads(result.stdout)
        nodes = {n['id']: n for n in metadata['resolve']['nodes']}
        by_id = {p['id']: p for p in metadata['packages']}
        roots = [p['id'] for p in metadata['packages'] if p['name'] == provider['package']]
        if len(roots) != 1:
            raise ValueError('selected image must identify exactly one Cargo package')
        todo, visited = roots[:], set()
        while todo:
            identity = todo.pop()
            if identity in visited:
                continue
            visited.add(identity)
            item = by_id[identity]
            todo.extend(edge['pkg'] for edge in nodes[identity]['deps']
                        if any(kind['kind'] != 'dev' for kind in edge['dep_kinds']))
            if item['source'] is None:
                manifest = Path(item['manifest_path']).resolve()
                if not manifest.is_relative_to(ROOT):
                    raise ValueError('selected image has a local package outside the source checkout')
                paths.add(manifest)
                # Cargo can inherit package license/version/dependency fields
                # from a workspace manifest above the selected path package.
                # Bind existing ancestors rather than reimplementing workspace
                # discovery or assuming metadata.workspace_root owns every node.
                for ancestor in manifest.parent.parents:
                    if not ancestor.is_relative_to(ROOT):
                        break
                    inherited = ancestor / 'Cargo.toml'
                    if inherited.is_file():
                        paths.add(inherited)
                for pattern in ('LICENSE*', 'COPYING*'):
                    paths.update(p for p in manifest.parent.glob(pattern) if p.is_file())
                if item.get('license_file'):
                    license_file = Path(item['license_file'])
                    if not license_file.is_absolute():
                        license_file = manifest.parent / license_file
                    paths.add(license_file.resolve())
                packages[str(manifest.relative_to(ROOT))] = {'name': item['name'], 'version': item['version']}
        paths.add(Path(metadata['workspace_root']) / 'Cargo.lock')
    for path in paths:
        if not path.resolve().is_relative_to(ROOT) or not path.is_file():
            raise ValueError(f'invalid selected notice input: {path}')
    inputs = [{'path': p.relative_to(ROOT).as_posix(), 'sha256': digest(p)} for p in sorted(paths)]
    return inputs, packages


def prepare(provider_path):
    provider_path = provider_path.resolve()
    provider = generate.image_provider(provider_path)
    if not provider_path.is_relative_to(ROOT):
        raise ValueError('the notice provider must be owned by this source checkout')
    runtime = notice_generator.runtime_notices.evidence()
    inputs, packages = selected_inputs(provider_path, provider, runtime)
    selection = {'schemaVersion': 1, 'provider': provider['id'], 'libraryName': provider['libraryName']}
    return provider, runtime, {
        'schemaVersion': 1, 'selection': selection,
        'provider': provider_path.relative_to(ROOT).as_posix(),
        'manifest': provider['manifest'].relative_to(ROOT).as_posix(),
        'features': provider.get('features', []), 'targets': runtime['source']['targets'],
        'inputs': inputs, 'local_packages': packages, 'runtime_archives': runtime,
        'scope': 'Rust license material follows the selected image closure for the supported runtime targets. '
                 'Runtime archives are required separately installed peers. Other declared peers are not '
                 'asserted to be embedded. Native artifact hashes belong to the package qualification receipt.',
    }


def render(provider, runtime):
    manifest = provider['manifest'].relative_to(ROOT).as_posix()
    graphs = [(provider['libraryName'] + ' ' + target, manifest, target) for target in runtime['source']['targets']]
    features = {name: tuple(provider.get('features', ())) for name, _, _ in graphs}
    version, notices = notice_generator.collect_rust_notices(graphs, features)
    lines = ['# Selected SDK image third-party notices', '',
             f"Image: `{provider['libraryName']}`. Generated with `{version}` from locked Cargo graphs.",
             'This document records attribution, not approval of unresolved runtime policy. '
             'See third-party-sources.json for exact image selection, inputs and runtime archive/source custody.', '',
             '## Selected Rust graphs', '']
    lines.extend(f'- `{target}`: `{manifest}`, shipping features {json.dumps(provider.get("features", []))}.'
                 for _, _, target in graphs)
    lines.extend(notice_generator.runtime_notices.declaration_lines(runtime))
    for item in runtime['packages']:
        lines.extend(['', f"### Original upstream notice: {item['name']}", '', '```text',
                      item['license_notice'].rstrip(), '```'])
    lines.extend(['', '## Selected native license texts', ''])
    lines.extend(notice_generator.license_text_lines(notices))
    return '\n'.join(lines).rstrip() + '\n'


def check_inputs(destination, provider_path=None):
    sources = destination / SOURCES
    if not sources.is_file() or not (destination / NOTICE).is_file():
        raise ValueError('selected SDK attribution is missing; generate package notices')
    saved = json.loads(sources.read_text())
    provider_path = provider_path or ROOT / saved['provider']
    _, _, expected = prepare(provider_path)
    expected['notice_sha256'] = digest(destination / NOTICE)
    if saved != expected:
        raise ValueError('selected SDK attribution inputs or notice bytes changed; regenerate notices')
    selection = json.loads((destination / 'native-image.json').read_text())
    if selection != saved['selection']:
        raise ValueError('SDK attribution belongs to a different selected native image')
    return saved


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--provider', type=Path)
    parser.add_argument('--destination', type=Path, default=PACKAGE)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--write', action='store_true')
    mode.add_argument('--check-inputs', action='store_true')
    args = parser.parse_args()
    if args.check_inputs and args.provider is None and (args.destination / SOURCES).is_file():
        args.provider = ROOT / json.loads((args.destination / SOURCES).read_text())['provider']
    destination = generate.selected_destination(
        generate.image_provider((args.provider or PACKAGE / 'providers/default.json').resolve()), args.destination)
    if args.check_inputs:
        check_inputs(destination, args.provider)
        print('Selected SDK attribution inputs match')
        return
    provider, runtime, record = prepare((args.provider or PACKAGE / 'providers/default.json').resolve())
    text = render(provider, runtime)
    record['notice_sha256'] = hashlib.sha256(text.encode()).hexdigest()
    files = {NOTICE: text, SOURCES: json.dumps(record, indent=2) + '\n'}
    for name, content in files.items():
        path = destination / name
        if args.write:
            path.write_text(content)
        elif not path.is_file() or path.read_text() != content:
            raise ValueError(f'selected SDK attribution differs: {name}')
    print('Selected SDK attribution ' + ('written' if args.write else 'reproduced'))


if __name__ == '__main__':
    try:
        main()
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        raise SystemExit(f'SDK attribution: {error}') from error
