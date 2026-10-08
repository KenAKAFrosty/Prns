"""Materialize a disposable SDK package from its maintained publishable sources."""
import json
from pathlib import Path

MARKER = '.prns-sdk-staging.json'


def destination_path(source, destination):
    source = source.resolve()
    destination = destination.absolute()
    if any(path.is_symlink() for path in (destination, *destination.parents)):
        raise ValueError('SDK destination must not contain symlinks')
    destination = destination.resolve()
    if destination == source:
        return destination
    if destination.is_relative_to(source) or source.is_relative_to(destination):
        raise ValueError('SDK destination must not overlap the source package')
    if destination.exists() and not (destination / MARKER).is_file() and any(destination.iterdir()):
        raise ValueError('SDK destination is not an owned staging package; choose an empty directory')
    return destination


def source_files(source, template=None):
    template = template or source
    package = json.loads((template / 'package.json').read_text())
    ownership = json.loads((template / 'outputs.json').read_text())
    generated = [Path(value) for value in (*ownership['directories'], *ownership['files'])]
    files = {}
    for pattern in package['files']:
        for match in source.glob(pattern):
            if match.is_symlink():
                raise ValueError(f'symlink in SDK package source: {match}')
            for path in match.rglob('*') if match.is_dir() else (match,):
                relative = path.relative_to(source)
                if (any(relative == owned or relative.is_relative_to(owned) for owned in generated)
                        or any(part.endswith('.xcframework') for part in relative.parts)
                        or relative.is_relative_to('android/src/main/jniLibs')):
                    continue
                if path.is_symlink():
                    raise ValueError(f'symlink in SDK package source: {relative}')
                if path.is_file():
                    files[relative] = path.read_bytes()
    # A stage is an installable runtime package, not another development checkout.
    # Its peer contract is unchanged and contains no source-relative dependencies.
    package.pop('devDependencies', None)
    package.pop('scripts', None)
    files[Path('package.json')] = (json.dumps(package, indent=2) + '\n').encode()
    return files


def synchronize(source, destination, check):
    destination = destination_path(source, destination)
    if destination == source.resolve():
        return destination
    files = source_files(source)
    marker = destination / MARKER
    previous = set()
    if marker.exists():
        document = json.loads(marker.read_text())
        if document.get('schemaVersion') != 1:
            raise ValueError('unsupported SDK staging ownership version')
        for name in document['files']:
            relative = Path(name)
            if relative.is_absolute() or '..' in relative.parts or relative.as_posix() != name:
                raise ValueError('invalid SDK staging ownership path')
            previous.add(relative)
    for relative in previous | set(files) | {Path(MARKER)}:
        path = destination / relative
        if any(part.is_symlink() for part in (path, *path.parents)):
            raise ValueError(f'symlink in SDK staging source: {relative}')
    obsolete = (previous | set(source_files(destination, source))) - set(files)
    files[Path(MARKER)] = (json.dumps({'schemaVersion': 1, 'files': sorted(map(str, files))}, indent=2) + '\n').encode()
    changed = {path for path, content in files.items()
               if not (destination / path).is_file() or (destination / path).read_bytes() != content}
    if check and (changed or obsolete):
        raise ValueError('SDK staging differs; regenerate the selected binding recipe:\n' +
                         '\n'.join(map(str, sorted(changed | obsolete))))
    if not check:
        for path in obsolete:
            (destination / path).unlink(missing_ok=True)
        for path in changed:
            output = destination / path
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(files[path])
    return destination
