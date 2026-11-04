"""Verify preserved POM evidence against the app's actual Gradle closure.

POM declarations are evidence, not license approval. Keeping this distinction
allows reproducible notices while a separate manual review keeps the release gate closed.
"""
from __future__ import annotations
import hashlib
import gzip
import io
import importlib.util
import json
from pathlib import Path
import xml.etree.ElementTree as ET

BASELINE = 'applications/tools/native-bindings/android-maven-baseline.json'
EVIDENCE = 'release/licenses/app-maven/pom-license-evidence.json'
ARTIFACTS = 'release/licenses/app-maven/artifact-notices.json'
MAX_NOTICE_BYTES = 4 * 1024 * 1024


def _unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f'Duplicate Maven evidence key: {key}')
        result[key] = value
    return result


def _source(root: Path, source: dict) -> bytes:
    directory = (root / EVIDENCE).parent
    path = directory / source['path']
    if not path.resolve().is_relative_to(directory.resolve()):
        raise ValueError('Maven notice source escapes its evidence directory')
    packed = path.read_bytes()
    if len(packed) > MAX_NOTICE_BYTES or hashlib.sha256(packed).hexdigest() != source['file_sha256']:
        raise ValueError('Maven notice source file changed or exceeds its bound')
    if source['encoding'] == 'gzip':
        with gzip.GzipFile(fileobj=io.BytesIO(packed)) as stream:
            raw = stream.read(MAX_NOTICE_BYTES + 1)
    elif source['encoding'] == 'plain':
        raw = packed
    else:
        raise ValueError('Unknown Maven notice encoding')
    if (len(raw) > MAX_NOTICE_BYTES or len(raw) != source['bytes']
            or hashlib.sha256(raw).hexdigest() != source['sha256']):
        raise ValueError('Maven notice source bytes changed')
    return raw


def notice_ranges(index: bytes, contents: bytes) -> list[tuple[str, str]]:
    """Read the archive's byte offsets, without treating component labels as licenses."""
    entries = json.loads(index, object_pairs_hook=_unique_object)
    if not isinstance(entries, dict):
        raise ValueError('Maven notice index must be an object')
    result, intervals = [], []
    for name, item in sorted(entries.items()):
        if not name or not isinstance(item, dict) or set(item) != {'start', 'length'}:
            raise ValueError('Malformed Maven notice range')
        start, length = item['start'], item['length']
        if (type(start) is not int or type(length) is not int or start < 0
                or length <= 0 or start + length > len(contents)):
            raise ValueError('Maven notice range lies outside source bytes')
        intervals.append((start, start + length))
        result.append((name, contents[start:start + length].decode('utf-8')))
    ordered = sorted(intervals)
    if any(left[1] > right[0] for left, right in zip(ordered, ordered[1:])):
        raise ValueError('Maven notice ranges overlap')
    if not entries and contents.strip():
        raise ValueError('Maven notice text has no declared ranges')
    return result


def packaged_notices(root: Path, baseline: dict, documents: dict) -> dict:
    directory = (root / EVIDENCE).parent
    manifest = json.loads((root / ARTIFACTS).read_text(), object_pairs_hook=_unique_object)
    if manifest.get('schema') != 1:
        raise ValueError('Unsupported Maven artifact notice schema')
    bound = {}
    for kind in ('resolution', 'pom_origins'):
        item = manifest[kind]
        path = directory / item['path']
        if not path.resolve().is_relative_to(directory.resolve()):
            raise ValueError('Maven custody source escapes its evidence directory')
        raw = path.read_bytes()
        if hashlib.sha256(raw).hexdigest() != item['sha256']:
            raise ValueError(f'Maven {kind} custody source changed')
        bound[kind] = json.loads(raw, object_pairs_hook=_unique_object)
    resolution, origins = bound['resolution'], bound['pom_origins']
    if resolution['configurations'] != baseline:
        raise ValueError('Maven artifact resolution differs from selected baseline')
    if (origins.get('schema') != 1 or origins['pom_evidence_sha256']
            != hashlib.sha256((root / EVIDENCE).read_bytes()).hexdigest()):
        raise ValueError('Maven origin evidence does not bind the preserved POMs')
    seen_origins = set()
    for origin in origins['documents']:
        coordinate = origin['coordinate']
        if coordinate in seen_origins or coordinate not in documents:
            raise ValueError('Duplicate or foreign Maven POM origin')
        seen_origins.add(coordinate)
        document = documents[coordinate]
        if origin['status'] == 'exact-pom-match':
            verified = origin['verified']
            group, artifact, version = coordinate.split(':')
            suffix = '/'.join((group.replace('.', '/'), artifact, version, f'{artifact}-{version}.pom'))
            allowed = {host + suffix for host in (
                'https://dl.google.com/dl/android/maven2/', 'https://repo.maven.apache.org/maven2/')}
            if (verified['url'] not in allowed or verified['resolved_url'] not in allowed
                    or verified['sha256'] != document['pom_sha256']):
                raise ValueError('Maven POM origin coordinate or bytes disagree')
        elif origin['status'] == 'existing-integrity-bound-npm-origin':
            if (origin['source'] != document['source']
                    or origin['pom_sha256'] != document['pom_sha256']):
                raise ValueError('Maven npm origin disagrees with preserved evidence')
        else:
            raise ValueError('Unverified Maven POM origin')
    if seen_origins != set(documents):
        raise ValueError('Maven POM origin coverage is incomplete')
    expected = {}
    for profile, artifacts in resolution['artifacts'].items():
        if profile not in baseline:
            raise ValueError('Maven artifact has an unselected profile')
        seen = set()
        for artifact in artifacts:
            key = artifact['coordinate'], artifact['file_name'], artifact['sha256']
            if key in seen or key[0] not in baseline[profile]:
                raise ValueError('Duplicate or foreign resolved Maven artifact')
            seen.add(key)
            expected.setdefault(key, set()).add(profile)
    if set(resolution['artifacts']) != set(baseline):
        raise ValueError('Missing selected Maven artifact profile')
    seen, notices = set(), []
    for artifact in manifest['artifacts']:
        key = artifact['coordinate'], artifact['file_name'], artifact['sha256']
        if key in seen or key not in expected or set(artifact['profiles']) != expected[key]:
            raise ValueError('Maven notice artifact differs from resolved custody')
        seen.add(key)
        if artifact['source_observation'] == 'integrity-checked npm archive':
            source = documents[key[0]]['source']
            if (source['kind'] != 'npm-local-maven-repository'
                    or artifact['npm_integrity'] != source['integrity']
                    or artifact['npm_resolved'] != source['resolved']
                    or Path(artifact['npm_member']).name != key[1]):
                raise ValueError('Maven packaged notice npm custody disagrees')
        elif artifact['source_observation'] != 'existing Gradle cache':
            raise ValueError('Unknown Maven artifact observation')
        sources = {}
        for item in artifact['notices']:
            path = item['archive_path']
            if path in sources:
                raise ValueError('Duplicate Maven archive notice path')
            sources[path] = _source(root, item['source'])
        pairs = []
        if 'third_party_licenses.json' in sources:
            if 'third_party_licenses.txt' not in sources:
                raise ValueError('Maven notice range index is missing its text')
            pairs.extend(notice_ranges(sources.pop('third_party_licenses.json'),
                                       sources.pop('third_party_licenses.txt')))
        for path, raw in sorted(sources.items()):
            if raw.strip():
                pairs.append((path, raw.decode('utf-8')))
        for name, text in pairs:
            notices.append({'coordinate': key[0], 'artifact': key[1], 'name': name,
                            'text': text, 'profiles': sorted(expected[key])})
    if seen != set(expected):
        raise ValueError('Maven artifact notice coverage is incomplete')
    return {'artifacts': len(seen), 'pom_origins': len(seen_origins),
            'notices': sorted(notices, key=lambda item: (item['coordinate'], item['artifact'], item['name']))}


def inputs(root: Path) -> tuple[Path, ...]:
    return (root / BASELINE, root / '.gitattributes', root / 'validation/security/app_maven.py', root / 'applications/tools/native-bindings/android_maven.py',
            *(p for p in sorted((root/'release/licenses/app-maven').rglob('*')) if p.is_file()))


def report(root: Path) -> dict:
    spec = importlib.util.spec_from_file_location('app_android_maven', root/'applications/tools/native-bindings/android_maven.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    baseline = module.coordinates(json.loads((root/BASELINE).read_text()))
    union = sorted(set().union(*baseline.values()))
    evidence = json.loads((root/EVIDENCE).read_text())
    if evidence.get('schema_version') != 1 or evidence.get('runtime_coordinates') != union:
        raise ValueError('App Maven POM evidence differs from the resolved runtime baseline')
    documents = evidence['documents']
    for coordinate, item in documents.items():
        path = (root/EVIDENCE).parent / item['source']['evidence_copy']
        if not path.resolve().is_relative_to((root/EVIDENCE).parent.resolve()):
            raise ValueError('Maven evidence path escapes its directory')
        raw = path.read_bytes()
        if hashlib.sha256(raw).hexdigest() != item['pom_sha256']:
            raise ValueError(f'Maven POM evidence changed: {coordinate}')
        xml = ET.fromstring(raw)
        parent = xml.find('{*}parent')
        parent_parts = [parent.findtext('{*}'+key, default='').strip() if parent is not None else ''
                        for key in ('groupId', 'artifactId', 'version')]
        parent_coordinate = ':'.join(parent_parts) if parent is not None else None
        own_parts = [(xml.findtext('{*}groupId') or parent_parts[0]).strip(),
                     (xml.findtext('{*}artifactId') or '').strip(),
                     (xml.findtext('{*}version') or parent_parts[2]).strip()]
        if not all(own_parts) or ':'.join(own_parts) != coordinate or item['coordinate'] != coordinate:
            raise ValueError(f'Maven coordinate disagrees with preserved POM: {coordinate}')
        if parent_coordinate != item['parent']:
            raise ValueError(f'Maven parent disagrees with preserved POM: {coordinate}')
        licenses = [{key: value for key in ('name', 'url', 'distribution', 'comments')
                     if (value := node.findtext(f'{{*}}{key}')) is not None}
                    for node in xml.findall('{*}licenses/{*}license')]
        if licenses != item['declared_licenses']:
            raise ValueError(f'Maven license declarations disagree with preserved POM: {coordinate}')
    rows = []
    for coordinate in union:
        current, visited = coordinate, set()
        while True:
            if current in visited or current not in documents:
                raise ValueError(f'Missing/cyclic Maven license parent: {coordinate}')
            visited.add(current)
            item = documents[current]
            if item['declared_licenses']:
                break
            current = item['parent']
        if documents[coordinate]['nearest_license_declarations_at'] != current:
            raise ValueError(f'Maven license inheritance changed: {coordinate}')
        rows.append({'coordinate':coordinate,'declarations':item['declared_licenses'],
                     'declared_at':current,'pom_sha256':documents[coordinate]['pom_sha256'],
                     'provenance':documents[coordinate]['source']})
    attribution = packaged_notices(root, baseline, documents)
    # These named declarations make the existing manual review actionable. They
    # are not an exhaustive license classifier or a selection among alternatives.
    review_terms = ('ML Kit Terms of Service', 'Android Software Development Kit License',
                    'LGPL-2.1-or-later')
    declarations = []
    for name in review_terms:
        coordinates = [row['coordinate'] for row in rows
                       if any(item.get('name') == name for item in row['declarations'])]
        if coordinates:
            declarations.append({'name': name, 'coordinates': coordinates})
    return {'schema':2,'configurations':baseline,'packages':rows,
            'artifact_evidence': {key: attribution[key] for key in ('artifacts', 'pom_origins')},
            'notices': attribution['notices'],
            'manual_review': {
                'summary': 'Complete artifact-origin, bundled-attribution and license-policy review '
                           'remains pending for the selected Maven closure. Verified evidence is '
                           'not release approval; the named declarations do not select license alternatives.',
                'declarations': declarations}}


def audit(report: dict) -> None:
    review = report['manual_review']
    details = [review['summary']]
    details.extend(f"{item['name']}: {', '.join(item['coordinates'])}"
                   for item in review['declarations'])
    raise ValueError('App Maven manual release-policy review pending:\n' + '\n'.join(details))
