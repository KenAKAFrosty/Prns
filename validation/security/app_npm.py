"""The app's locked production npm closure, including source-linked packages.

The lock's `dev` bit is not a reachability proof: workspace peers may be marked
as development dependencies at the installer root. Start from the application
and follow runtime and peer edges instead. The staged SDK is represented by its
owning source manifest, never by ignored output or installed node_modules.
"""
from __future__ import annotations

import base64
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import tarfile
import tempfile
import urllib.request

try:
    from .npm_lockfile import has_registry_integrity
except ImportError:  # Direct audit entrypoint.
    from npm_lockfile import has_registry_integrity

SOURCE_MANIFESTS = {
    'prns/app': 'applications/prns/app/package.json',
    'prns/bindings': 'applications/prns/bindings/package.json',
    'prns/platform': 'applications/prns/platform/package.json',
    'target/react-native-sdk': 'prns-react-native/package.json',
}
DEPENDENCY_FIELDS = ('dependencies', 'optionalDependencies', 'peerDependencies', 'peerDependenciesMeta')


def app_inputs(root: Path) -> tuple[Path, ...]:
    return tuple(root / path for path in (
        'applications/package.json', 'applications/package-lock.json',
        'prns-js/package.json', 'prns-js/package-lock.json',
        *SOURCE_MANIFESTS.values(),
        'tools/uniffi/react-native.json',
        'prns-react-native/patches/@ubjs+react-native+0.31.0-6.patch',
    ))


def _name(path: str, metadata: dict) -> str:
    return metadata.get('name') or path.rsplit('node_modules/', 1)[-1]


def app_closure(root: Path) -> dict:
    locks = {}
    for project in ('applications', 'prns-js'):
        lock = json.loads((root / project / 'package-lock.json').read_text())
        if lock.get('lockfileVersion') != 3 or not isinstance(lock.get('packages'), dict):
            raise ValueError(f'{project}: expected npm lockfile v3 package map')
        locks[project] = lock['packages']
        manifest = json.loads((root / project / 'package.json').read_text())
        for field in (*DEPENDENCY_FIELDS, 'devDependencies', 'workspaces'):
            if manifest.get(field, {}) != lock['packages'][''].get(field, {}):
                raise ValueError(f'{project}: package.json and locked {field} disagree')
    for path, source in SOURCE_MANIFESTS.items():
        actual = json.loads((root / source).read_text())
        locked = locks['applications'].get(path)
        if not isinstance(locked, dict):
            raise ValueError(f'app lock lacks source package {path}')
        for field in ('name', *DEPENDENCY_FIELDS):
            if actual.get(field, {}) != locked.get(field, {}):
                raise ValueError(f'{source}: staged/locked {field} disagrees with owning manifest')

    def resolve(project: str, requester: str, name: str) -> str | None:
        parent = PurePosixPath(requester)
        while True:
            path = str(parent / 'node_modules' / name)
            if path in locks[project]:
                return path
            if str(parent) == '.':
                return None
            parent = parent.parent

    pending = [('applications', 'prns/app')]
    visited = set()
    packages = {}
    omitted = set()
    optional_peers = []
    missing_peers = set()
    while pending:
        project, path = pending.pop()
        if (project, path) in visited:
            continue
        visited.add((project, path))
        metadata = locks[project][path]
        if metadata.get('link'):
            target = metadata.get('resolved')
            if project == 'applications' and target == '../prns-js':
                pending.append(('prns-js', ''))
            elif project == 'applications' and target in SOURCE_MANIFESTS:
                pending.append((project, target))
            else:
                raise ValueError(f'{project}/{path}: unreviewed source link {target!r}')
            continue
        source = project == 'prns-js' and path == '' or project == 'applications' and path in SOURCE_MANIFESTS
        if not source:
            name = _name(path, metadata)
            if not metadata.get('version'):
                raise ValueError(f'{project}/{path}: reachable package has no locked version')
            resolved = metadata.get('resolved', '')
            record = {field: metadata.get(field) for field in ('version', 'license', 'resolved', 'integrity')}
            record.update(name=name, locations=[f'{project}/{path}'])
            if not has_registry_integrity(metadata):
                raise ValueError(f'{project}/{path}: package lacks exact registry source/integrity')
            identity = (name, record['version'], record['resolved'], record['integrity'])
            if identity in packages:
                if packages[identity]['license'] != record['license']:
                    raise ValueError(f'{name}: inconsistent license for the same locked archive')
                packages[identity]['locations'].extend(record['locations'])
            else:
                packages[identity] = record
        required = metadata.get('dependencies', {})
        optional = metadata.get('optionalDependencies', {})
        peers = metadata.get('peerDependencies', {})
        peer_meta = metadata.get('peerDependenciesMeta', {})
        for name in sorted(set(required) | set(optional) | set(peers)):
            target = resolve(project, path, name)
            # Source-linked library development locks install optional frameworks
            # that the app has not selected. Registry packages are different:
            # installed optional peers can auto-enable behavior at runtime.
            optional_peer = name not in required and name not in optional and peer_meta.get(name, {}).get('optional') is True
            absent = target is None or locks[project][target] == {'optional': True}
            if optional_peer and (source or absent):
                optional_peers.append((project, path, name, target))
                continue
            nullable = name in optional
            # Bare optional placeholders have no resolved archive to ship.
            if absent:
                if not nullable:
                    # A package may bundle a capability whose peer is not installed.
                    # Preserve that unsatisfied declaration for the release audit;
                    # it supplies no archive bytes to collect into notices.
                    if name in peers and name not in required:
                        missing_peers.add((project, path, name, peers[name]))
                        continue
                    raise ValueError(f'{project}/{path}: missing required runtime dependency {name}')
                omitted.add((project, path, name))
            else:
                pending.append((project, target))
    ordered = sorted(packages.values(), key=lambda p: (p['name'], p['version'], p['resolved']))
    for package in ordered:
        package['locations'].sort()
    return {'schema': 1, 'root': 'applications/prns/app', 'packages': ordered,
            'unresolved_optional': [list(value) for value in sorted(omitted)],
            'unresolved_required_peers': [list(value) for value in sorted(missing_peers)],
            'unselected_optional_peers': [list(value[:3]) for value in sorted(optional_peers, key=str)
                                          if value[3] is None or (value[0], value[3]) not in visited]}


def _verify_integrity(content: bytes, integrity: str) -> None:
    if not isinstance(integrity, str) or not integrity.startswith('sha512-'):
        raise ValueError('npm archive lacks SHA-512 integrity')
    try:
        expected = base64.b64decode(integrity[7:], validate=True)
    except ValueError as error:
        raise ValueError('invalid npm archive integrity') from error
    if len(expected) != 64 or hashlib.sha512(content).digest() != expected:
        raise ValueError('npm archive differs from locked integrity')


def package_archive(root: Path, package: dict, cache: Path) -> Path:
    resolved = package['resolved']
    if not resolved.startswith('https://registry.npmjs.org/'):
        raise ValueError('unreviewed npm archive source')
    cache.mkdir(parents=True, exist_ok=True)
    name = base64.b64decode(package['integrity'][7:], validate=True).hex() + '.tgz'
    path = cache / name
    if path.exists():
        _verify_integrity(path.read_bytes(), package['integrity'])
        return path
    with urllib.request.urlopen(resolved, timeout=60) as response:
        content = response.read(100 * 1024 * 1024 + 1)
    if len(content) > 100 * 1024 * 1024:
        raise ValueError('npm notice archive exceeds 100 MiB')
    _verify_integrity(content, package['integrity'])
    with tempfile.NamedTemporaryFile(dir=cache, delete=False) as temporary:
        temporary.write(content)
        staged = Path(temporary.name)
    staged.replace(path)
    return path


def package_notices(archive: Path) -> str:
    notices = []
    with tarfile.open(archive, 'r:gz') as source:
        for member in source:
            path = PurePosixPath(member.name)
            name = path.name.upper()
            if not member.isfile() or not any(name == prefix or name.startswith(prefix + '.') or name.startswith(prefix + '-')
                    for prefix in ('LICENSE', 'LICENCE', 'NOTICE', 'COPYING', 'COPYRIGHT', 'UNLICENSE')) and '.LICENSE.' not in name:
                continue
            if member.size > 4 * 1024 * 1024:
                raise ValueError('npm notice exceeds 4 MiB')
            stream = source.extractfile(member)
            if stream is None:
                raise ValueError("npm notice member cannot be read")
            text = stream.read().decode('utf-8-sig').strip()
            if text:
                notices.append((member.name, text))
    if not notices:
        raise ValueError(f'package archive contains no license/notice text: {archive.name}')
    return '\n\n'.join(f'{name}\n\n{text}' for name, text in sorted(notices))


def audit_advisories(closure: dict, output: Path) -> None:
    """Let pinned npm match advisories for every reachable exact package version.

    The temporary graph makes all reviewed identities direct production entries,
    including workspace peers npm marked `dev` and source-linked dependencies.
    It performs no install/resolution and never changes a source lockfile.
    """
    dependencies = {}
    packages = {}
    for index, package in enumerate(closure['packages']):
        alias = f'audited-{index}'
        dependencies[alias] = f'npm:{package["name"]}@{package["version"]}'
        packages[f'node_modules/{alias}'] = {key: package[key] for key in ('name', 'version', 'resolved', 'integrity', 'license')}
    manifest = {'name': 'prns-production-audit', 'version': '0.0.0', 'private': True, 'dependencies': dependencies}
    lock = {'name': manifest['name'], 'version': manifest['version'], 'lockfileVersion': 3,
            'requires': True, 'packages': {'': manifest, **packages}}
    with tempfile.TemporaryDirectory(prefix='prns-app-npm-audit-') as temporary:
        path = Path(temporary)
        (path/'package.json').write_text(json.dumps(manifest))
        (path/'package-lock.json').write_text(json.dumps(lock))
        before = (path/'package-lock.json').read_bytes()
        result = subprocess.run(['npm', 'audit', '--package-lock-only', '--ignore-scripts', '--omit=dev', '--json'],
                                cwd=path, text=True, capture_output=True, timeout=180)
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(result.stdout)
        output.with_suffix('.stderr.log').write_text(result.stderr)
        if (path/'package-lock.json').read_bytes() != before:
            raise ValueError('npm audit unexpectedly changed its exact-version input')
        if result.returncode:
            raise ValueError(f'app npm advisory audit failed ({result.returncode}); see {output}')


def notice_inputs(root: Path) -> tuple[Path, ...]:
    directory = root / 'release/licenses/app-npm'
    return (*app_inputs(root), root / 'validation/security/app_npm.py', root / 'validation/security/npm_lockfile.py',
            root / 'deny.toml', *(p for p in sorted(directory.rglob('*')) if p.is_file()))


def license_permitted(expression: str, allowed: set[str]) -> bool:
    """Evaluate SPDX AND/OR/WITH without approving a new license identifier."""
    import re
    if not isinstance(expression, str):
        return False
    tokens = re.findall(r"[A-Za-z0-9.+-]+|[()]", expression)
    if ''.join(tokens) != re.sub(r"\s+", '', expression):
        return False
    index = 0

    def atom():
        nonlocal index
        if index >= len(tokens):
            raise ValueError('missing SPDX operand')
        name = tokens[index]
        index += 1
        if name == '(':
            value = disjunction()
            if index >= len(tokens) or tokens[index] != ')':
                raise ValueError('unclosed SPDX expression')
            index += 1
            return value
        if name in {'AND', 'OR', 'WITH', ')'}:
            raise ValueError('invalid SPDX operand')
        if index < len(tokens) and tokens[index] == 'WITH':
            index += 1
            if index >= len(tokens) or tokens[index] in {'AND', 'OR', 'WITH', '(', ')'}:
                raise ValueError('missing SPDX exception')
            name += ' WITH ' + tokens[index]
            index += 1
        return name in allowed

    def conjunction():
        nonlocal index
        value = atom()
        while index < len(tokens) and tokens[index] == 'AND':
            index += 1
            right = atom()
            value = value and right
        return value

    def disjunction():
        nonlocal index
        value = conjunction()
        while index < len(tokens) and tokens[index] == 'OR':
            index += 1
            right = conjunction()
            value = value or right
        return value

    try:
        result = disjunction()
        return index == len(tokens) and result
    except (ValueError, RecursionError):
        return False


def notice_report(root: Path, cache: Path) -> dict:
    """Collect verified package text, retaining unresolved attribution as evidence.

    Successful collection is not release approval. The audit below independently
    rejects unresolved source attribution and unreviewed license declarations.
    """
    import tomllib
    closure = app_closure(root)
    source = json.loads((root / 'release/licenses/app-npm/provenance.json').read_text())
    if source.get('schema') != 1:
        raise ValueError('unsupported npm license provenance schema')
    fallback = {}
    for item in source['packages']:
        key = item['name'], item['version'], item['integrity']
        if key in fallback:
            raise ValueError('duplicate npm notice provenance')
        if not isinstance(item.get('texts'), list) or not isinstance(item.get('unresolved'), list):
            raise ValueError('malformed npm license fallback evidence')
        fallback[key] = item
    # Reuse the existing permissive policy, including allowed alternatives and
    # conjunctions. Cargo package exceptions do not implicitly approve npm.
    allowed = set(tomllib.loads((root/'deny.toml').read_text())['licenses']['allow'])
    records = []
    for package in closure['packages']:
        archive = package_archive(root, package, cache)
        key = package['name'], package['version'], package['integrity']
        evidence = fallback.get(key)
        texts, issues = [], []
        try:
            texts.append({'source': package['resolved'], 'text': package_notices(archive)})
        except ValueError as error:
            if evidence is None:
                issues.append(str(error).split(':', 1)[0])
        if evidence is not None:
            issues.extend(evidence['unresolved'])
            for item in evidence['texts']:
                path = root / item['path']
                if not path.resolve().is_relative_to((root/'release/licenses/app-npm').resolve()):
                    raise ValueError('npm notice provenance path escapes its evidence directory')
                raw = path.read_bytes()
                if hashlib.sha256(raw).hexdigest() != item['sha256']:
                    raise ValueError(f'npm license evidence changed: {item["path"]}')
                texts.append({'source': item['source'], 'text': raw.decode('utf-8-sig').strip()})
        policy = []
        if not license_permitted(package['license'], allowed):
            policy.append(f'Existing license policy does not permit {package["license"]!r}')
        if not texts and not issues:
            issues.append('No verified license text or unresolved attribution evidence is present')
        records.append(dict(package, texts=texts, attribution_issues=issues, policy_issues=policy,
                            provenance=evidence.get('provenance') if evidence else 'Integrity-checked published archive'))
    return dict(closure, packages=records)


def audit_notices(report: dict) -> None:
    failures = [f'{project}/{requester}: missing required peer {name}@{requirement}'
                for project, requester, name, requirement in report['unresolved_required_peers']]
    failures.extend(f'{p["name"]}@{p["version"]}: {issue}' for p in report['packages']
                    for issue in (*p['attribution_issues'], *p['policy_issues']))
    if failures:
        raise ValueError('App npm dependency/attribution/license review is unresolved:\n' + '\n'.join(failures))
