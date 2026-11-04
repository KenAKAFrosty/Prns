"""Attribution for published UniFFI npm packages and the maintained text patches."""
import hashlib
from pathlib import Path
import tomllib

import react_native

ROOT = react_native.ROOT


def input_paths():
    source = react_native.inputs()
    return tuple(sorted({
        Path(__file__).resolve(), Path(react_native.__file__).resolve(),
        react_native.LOCK, ROOT / 'deny.toml',
        ROOT / source['runtime_patch']['file'],
        *(react_native.HERE / item['file'] for item in source['patches']),
        *(react_native.HERE / item['license_file'] for item in source['packages']),
    }))


def evidence():
    source = react_native.inputs()
    allowed = tomllib.loads((ROOT / 'deny.toml').read_text())['licenses']['allow']
    packages = []
    for item in source['packages']:
        react_native.package_archive(item)
        notice = (react_native.HERE / item['license_file']).read_bytes()
        findings = []
        if item['license'] not in allowed:
            findings.append(f"Existing license policy does not permit {item['license']!r}; "
                            'crate-specific exceptions do not approve this npm package.')
        packages.append({
            'name': item['name'], 'version': item['version'],
            'archive': item['archive'], 'sha256': item['sha256'], 'integrity': item['integrity'],
            'license': item['license'], 'license_notice': notice.decode('utf-8'),
            'license_notice_sha256': hashlib.sha256(notice).hexdigest(),
            'upstream': source['upstream'], 'unresolved_policy': findings,
        })
    patch = source['runtime_patch']
    return {
        'packages': packages, 'source': source,
        'source_lock_sha256': react_native.digest(react_native.LOCK),
        'source_patches': [{'file': patch['file'], 'sha256': patch['sha256'],
                            'patch': (ROOT / patch['file']).read_text()}],
        'scope': 'These exact published npm packages are required runtime peers. '
                 'The SDK supplies a text patch for the React Native runtime; consumers '
                 'apply it before native compilation. Prebuilt Rust binaries are unchanged.',
        'approval': 'Attribution evidence is not release-policy approval. Upstream license '
                    'material is retained; package-specific Rust exceptions are not reused.',
    }


def declaration_lines(report):
    lines = ['', '## Selected UniFFI npm packages', '', report['scope'], '', report['approval'], '']
    for item in report['packages']:
        lines.append(f"- `{item['name']} {item['version']}` — `{item['license']}`; "
                     f"published archive SHA-256 `{item['sha256']}`.")
        lines.extend('  **Unresolved:** ' + finding for finding in item['unresolved_policy'])
    upstream = report['source']['upstream']
    lines.extend(['', f"Upstream source: {upstream['repository']} at `{upstream['revision']}`.",
                  f"Exact npm integrity, source licenses and patches are bound by configuration "
                  f"SHA-256 `{report['source_lock_sha256']}`."])
    return lines


def verify_policy(report):
    findings = [f"{item['name']}: {finding}" for item in report['packages']
                for finding in item['unresolved_policy']]
    if findings:
        raise ValueError('SDK runtime release review remains unresolved:\n' + '\n'.join(findings))
