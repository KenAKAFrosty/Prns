"""Current-source mobility evidence, explicitly separate from recorded-release qualification."""
from __future__ import annotations

import hashlib
import json
import os
import re
from pathlib import Path, PurePosixPath
import shutil
import stat
import subprocess
import sys
import tempfile
import tomllib

POLICY = Path('applications/release/working-tree-qualification.json')
EXCLUDED = {'node_modules', 'target', 'build', '.gradle', '.expo', '__pycache__', 'dist', 'dist-cjs', '.git'}


def load_policy(root):
    policy = json.loads((root / POLICY).read_text())
    if policy.get('schemaVersion') != 1 or policy.get('kind') != 'working-tree-source':
        raise ValueError('unsupported working-tree qualification policy')
    for key in ('sourceRoots', 'cargoEntrypoints'):
        values = policy.get(key)
        if not isinstance(values, list) or not values or len(values) != len(set(values)):
            raise ValueError(f'{key} must be a nonempty unique path list')
        for value in values:
            path = PurePosixPath(value)
            if path.is_absolute() or '..' in path.parts or not path.parts or str(path) != value:
                raise ValueError(f'unsafe {key} path: {value}')
    packages = policy.get('npmPackages')
    if not isinstance(packages, dict) or not packages:
        raise ValueError('npmPackages must identify reviewed source packages')
    for name, value in packages.items():
        if value not in policy['sourceRoots']:
            raise ValueError(f'unreviewed npm source root for {name}')
    return policy


def within(path, root):
    return path.resolve().is_relative_to(root.resolve())


def reviewed(path, root, policy):
    return any(within(path, root / scope) for scope in policy['sourceRoots'])


def inventory(root, policy):
    root = root.resolve()
    result = subprocess.run(['git', '-C', str(root), 'ls-files', '-z', '--cached', '--others',
                             '--exclude-standard', '--', *policy['sourceRoots']],
                            check=True, stdout=subprocess.PIPE)
    files = {}
    for encoded in sorted(set(result.stdout.split(b'\0'))):
        if not encoded:
            continue
        relative = Path(os.fsdecode(encoded))
        if EXCLUDED.intersection(relative.parts):
            continue
        source = root / relative
        # A removed tracked file is a deletion in the current snapshot.
        if not source.exists() and not source.is_symlink():
            continue
        if not reviewed(source, root, policy):
            raise ValueError(f'snapshot path escapes reviewed roots: {relative}')
        if source.is_symlink() or any(parent.is_symlink() for parent in source.parents if parent.is_relative_to(root)):
            raise ValueError(f'snapshot symlink is forbidden: {relative}')
        mode = source.stat().st_mode
        if not stat.S_ISREG(mode):
            raise ValueError(f'snapshot non-file is forbidden: {relative}')
        files[relative.as_posix()] = (hashlib.sha256(source.read_bytes()).hexdigest(), stat.S_IMODE(mode))
    return files


def snapshot(root, work, policy):
    before = inventory(root, policy)
    export = work / 'export'
    export.mkdir()
    for name, (expected, mode) in before.items():
        source = root / name
        content = source.read_bytes()
        if hashlib.sha256(content).hexdigest() != expected:
            raise ValueError(f'source changed during snapshot: {name}')
        destination = export / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(content)
        destination.chmod(mode)
    if before != inventory(root, policy):
        raise ValueError('source changed during snapshot; retry after edits settle')
    digest = hashlib.sha256(json.dumps(before, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
    receipt = {'schemaVersion': 1, 'kind': 'working-tree-source', 'sourceHead': subprocess.check_output(
        ['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip(),
        'snapshotSha256': digest, 'files': {name: {'sha256': sha, 'mode': mode} for name, (sha, mode) in before.items()},
        'releaseQualified': False, 'checks': []}
    (work / 'qualification.json').write_text(json.dumps(receipt, indent=2) + '\n')
    return export, receipt


def dependency_paths(document):
    for key, value in document.items():
        if key in ('dependencies', 'dev-dependencies', 'build-dependencies') and isinstance(value, dict):
            for name, dependency in value.items():
                if isinstance(dependency, dict) and 'path' in dependency:
                    yield name, dependency
        elif isinstance(value, dict):
            yield from dependency_paths(value)


def validate_dependencies(root, policy, mobility):
    """Traverse all declared Cargo edges, including optional/platform/dev edges."""
    root = root.resolve()
    pending = [root / path for path in policy['cargoEntrypoints']]
    manifests = set()
    packages = {}
    while pending:
        manifest = pending.pop().resolve()
        if manifest in manifests:
            continue
        if not reviewed(manifest, root, policy) or not manifest.is_file():
            raise ValueError(f'unreviewed or missing Cargo manifest: {manifest}')
        manifests.add(manifest)
        document = tomllib.loads(manifest.read_text())
        package = document.get('package', {})
        if package.get('name'):
            packages[package['name']] = manifest.relative_to(root).as_posix()
        for name, dependency in dependency_paths(document):
            target = (manifest.parent / dependency['path'] / 'Cargo.toml').resolve()
            if not reviewed(target, root, policy) or not target.is_file():
                raise ValueError(f'Cargo dependency {name} escapes source snapshot: {target}')
            identity = tomllib.loads(target.read_text()).get('package', {}).get('name')
            if identity != dependency.get('package', name):
                raise ValueError(f'Cargo dependency identity mismatch: {name} at {target}')
            pending.append(target)
        for member in document.get('workspace', {}).get('members', []):
            if '*' in member:
                raise ValueError('review wildcard workspace members before exporting them')
            pending.append(manifest.parent / member / 'Cargo.toml')
    for scope in ('applications', 'prns-react-native', 'prns-js'):
        for manifest in mobility.manifests_below(root / scope, 'package.json'):
            document = json.loads(manifest.read_text())
            for section in mobility.DEPENDENCY_SECTIONS:
                for name, selection in document.get(section, {}).items():
                    local = mobility.npm_local_path(selection) if isinstance(selection, str) else None
                    if local is None:
                        continue
                    target = (manifest.parent / local).resolve()
                    if within(target, root / 'applications'):
                        continue
                    expected = policy['npmPackages'].get(name)
                    if expected and target == (root / expected).resolve():
                        if json.loads((target / 'package.json').read_text()).get('name') != name:
                            raise ValueError(f'npm dependency identity mismatch: {name}')
                        continue
                    if mobility.shared_runtime_archive(target, root / 'applications', name):
                        continue
                    raise ValueError(f'unreviewed npm dependency {name}: {target}')
    for scope in policy['sourceRoots']:
        for directory, children, files in os.walk(root / scope, followlinks=False):
            children[:] = [child for child in children if child not in EXCLUDED]
            current = Path(directory)
            if '.npmrc' in files or (current.name == '.cargo' and {'config', 'config.toml'}.intersection(files)):
                raise ValueError(f'current-source qualification forbids resolver overrides: {current}')
    return dict(sorted(packages.items()))


def project_workspace(root, packages, receipt):
    """Retain inherited workspace policy without exporting unrelated firmware crates."""
    manifest = root / 'Cargo.toml'
    original = manifest.read_text()
    document = tomllib.loads(original)
    included = {str(Path(path).parent) for path in packages.values()}
    updated = original
    omitted = {}
    for key in ('members', 'default-members'):
        values = document.get('workspace', {}).get(key, [])
        retained = [value for value in values if value in included]
        omitted[key] = [value for value in values if value not in included]
        pattern = rf'(?ms)^{key}\s*=\s*\[.*?^\]'
        replacement = key + ' = ' + json.dumps(retained)
        updated, count = re.subn(pattern, lambda _: replacement, updated, count=1)
        if count != 1:
            raise ValueError(f'unsupported root workspace {key} declaration')
    parsed = tomllib.loads(updated)
    expected = json.loads(json.dumps(document))
    for key in omitted:
        expected['workspace'][key] = [value for value in document['workspace'][key] if value in included]
    if parsed != expected:
        raise ValueError('workspace projection changed more than reviewed membership')
    manifest.write_text(updated)
    receipt['workspaceProjection'] = {'reason': 'only the reviewed application and SDK dependency closure',
        'sourceSha256': hashlib.sha256(original.encode()).hexdigest(),
        'projectedSha256': hashlib.sha256(updated.encode()).hexdigest(), 'omitted': omitted}


def install_current_artifacts(root, environment, receipt, mobility):
    """Install actual current npm artifacts, preserving unrelated locked packages."""
    apps = root / 'applications'
    vendor = apps / 'vendor'
    vendor.mkdir(exist_ok=True)
    before = json.loads((apps / 'package-lock.json').read_text())
    artifacts = {}
    extra_packages = {}
    for name, scope in (('personal-rns', 'prns-js'), ('personal-rns-expo', 'prns-react-native')):
        packed = mobility.run(['npm', 'pack', '--ignore-scripts', '--json', '--pack-destination', str(vendor)],
                              cwd=root / scope, environment=environment, capture=True)
        report = json.loads(packed.stdout)
        if len(report) != 1:
            raise ValueError('npm pack must emit exactly one artifact')
        path = vendor / report[0]['filename']
        if not path.is_file() or path.parent != vendor:
            raise ValueError('npm pack returned an unsafe artifact path')
        metadata = mobility.packed_metadata(path)
        if metadata.get('name') != name:
            raise ValueError('packed npm identity does not match source policy')
        artifacts[name] = path
        for section in ('dependencies', 'optionalDependencies'):
            extra_packages.update(metadata.get(section, {}))
    changed_manifests = set()
    for manifest in mobility.manifests_below(apps, 'package.json'):
        package = json.loads(manifest.read_text())
        changed = False
        for section in mobility.DEPENDENCY_SECTIONS:
            for name, selection in package.get(section, {}).items():
                if name in artifacts and mobility.npm_local_path(selection) is not None:
                    package[section][name] = 'file:' + Path(os.path.relpath(artifacts[name], manifest.parent)).as_posix()
                    changed = True
        if changed:
            manifest.write_text(json.dumps(package, indent=2) + '\n')
            changed_manifests.add(manifest.parent.relative_to(apps).as_posix())
    mobility.run(['npm', 'install', '--package-lock-only', '--ignore-scripts', '--no-audit', '--no-fund'],
                 cwd=apps, environment=environment)
    after = json.loads((apps / 'package-lock.json').read_text())
    old, new = before['packages'], after['packages']
    allowed_source = {'../prns-js', '../prns-react-native'}
    if set(old) - set(new) != allowed_source & set(old):
        raise ValueError('current artifact installation removed unrelated locked packages')
    allowed_changed = changed_manifests | {'node_modules/' + name for name in artifacts}
    for key in set(old) & set(new):
        if key not in allowed_changed and not mobility.same_npm_package_content(old[key], new[key]):
            raise ValueError(f'current artifact installation changed unrelated locked package: {key}')
    for key in set(new) - set(old):
        name = key.removeprefix('node_modules/')
        if name not in extra_packages or new[key].get('version') != extra_packages[name]:
            raise ValueError(f'current artifact installation added an unreviewed package: {key}')
    for name, artifact in artifacts.items():
        entry = new.get('node_modules/' + name, {})
        if entry.get('link') or artifact.name not in entry.get('resolved', '') or not entry.get('integrity'):
            raise ValueError(f'current artifact is not content-locked: {name}')
    # Preserve unrelated original lock metadata after npm recomputes dev flags.
    for key in set(old) & set(new) - allowed_changed:
        after['packages'][key] = old[key]
    (apps / 'package-lock.json').write_text(json.dumps(after, indent=2) + '\n')
    mobility.run(['npm', 'ci', '--ignore-scripts', '--no-audit', '--no-fund'], cwd=apps, environment=environment)
    for name in artifacts:
        installed = apps / 'node_modules' / name
        if not installed.is_dir() or installed.is_symlink():
            raise ValueError(f'current npm package must be unpacked, not linked: {name}')
    receipt['artifacts'] = {name: {'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                                  'scope': 'current-source-code'} for name, path in artifacts.items()}


def qualify(root, keep_workspace, snapshot_only, mobility):
    policy = load_policy(root)
    temporary = tempfile.TemporaryDirectory(prefix='prns-working-tree-') if keep_workspace is None else None
    work = Path(temporary.name) if temporary else keep_workspace.resolve()
    if temporary is None:
        if work.exists():
            raise ValueError('--keep-workspace destination already exists')
        work.mkdir(parents=True)
    try:
        exported, receipt = snapshot(root, work, policy)
        receipt['cargoPackages'] = validate_dependencies(exported, policy, mobility)
        project_workspace(exported, receipt['cargoPackages'], receipt)
        receipt['checks'].append('source-snapshot-and-dependency-closure')
        if not snapshot_only:
            compatibility = mobility.load_compatibility()
            mobility.require_qualification_toolchains(compatibility, root)
            apps = exported / 'applications'
            environment = mobility.controlled_environment(work, apps, compatibility)
            # Deliberately do not invoke compatibility:check or rewrite its historical pin.
            # This mode attests current content, not an old commit or release artifact.
            checks = [
                ([sys.executable, '../tools/ubrn-vendor/vendor.py', 'check'], apps),
                (['npm', 'ci', '--ignore-scripts', '--no-audit', '--no-fund'], exported / 'prns-js'),
                (['npm', 'run', 'build:code'], exported / 'prns-js'),
                (['npm', 'ci', '--ignore-scripts', '--no-audit', '--no-fund'], exported / 'prns-react-native'),
                (['npm', 'run', 'verify'], exported / 'prns-react-native'),
                (['npm', 'run', 'bindings:test'], apps),
                (['npm', 'run', 'api:check'], apps),
                (['npm', 'run', 'native:test'], apps),
                (['npm', 'run', 'bindings:aggregate:test'], apps),
                (['npm', '--prefix', 'prns/platform', 'run', 'verify:code'], apps),
                # versions:check binds the historical JS artifact digest. Current
                # artifacts are checked above without claiming that release pin.
                *[(['npm', '--prefix', 'prns/app', 'run', gate], apps) for gate in
                  ('format:check', 'lint', 'typecheck', 'routes:check', 'config:check',
                   'expo:check', 'doctor', 'test', 'export:web')],
                (['npm', 'run', 'lxmf:verify'], apps),
            ]
            for command, cwd in checks:
                mobility.run(command, cwd=cwd, environment=environment)
                receipt['checks'].append(' '.join(command))
                if command == ['npm', 'run', 'build:code']:
                    install_current_artifacts(exported, environment, receipt, mobility)
                    receipt['checks'].append('install-content-locked-current-code-artifacts')
        (work / 'qualification.json').write_text(json.dumps(receipt, indent=2) + '\n')
        kind = 'SNAPSHOT' if snapshot_only else 'QUALIFICATION'
        print(f'APPLICATION_WORKING_TREE_{kind}_OK snapshot_sha256={receipt["snapshotSha256"]} release_qualified=false', flush=True)
        if keep_workspace:
            print(f'Current-source evidence retained at {work / "qualification.json"}', flush=True)
    finally:
        if temporary:
            temporary.cleanup()
