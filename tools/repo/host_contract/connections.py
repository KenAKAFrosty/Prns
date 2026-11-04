"""Project the owning native connection declarations and ordinary clients.

Only representation/ownership mappings live here. Fields, variants and client
methods are discovered from their actual Rust owners; unsupported syntax fails
rather than silently truncating the public contract.
"""
import copy
import hashlib
import json
import re
from .formatting import format_rust
from .remote_control import block, clean, fields, inventory, split

SOURCES = {
    'prns-host/impls/runtime/src/connections/model.rs': 'prns_host_runtime::connections',
    'prns-host/impls/runtime/src/connections/binding.rs': 'prns_host_runtime::connections',
    'prns-host/impls/runtime/src/attachment_scope.rs': 'prns_host_runtime::attachment_scope',
}
CLIENTS = {
    'Client': ('prns-host/impls/runtime/src/connections/mod.rs', 'ConnectionProfileClient'),
    'BindingClient': ('prns-host/impls/runtime/src/connections/binding.rs', 'ConnectionBindingClient'),
}
TRANSPORT = {'InterfaceConfig', 'InterfaceRoutingPolicy', 'Bitrate', 'CommandFailure'}
OPAQUE = {'Arc<redb::Error>', 'Arc<prns_storage::files::Error>', 'Arc<std::io::Error>'}
PRIMITIVES = {'String', 'bool', 'u8', 'u16', 'u32', 'u64', 'i8', 'i16', 'i32', 'i64', '()'}


def short(t):
    return re.sub(r'(?:crate::attachment_scope::|crate::owner::)', '', re.sub(r'\s+', '', t))


def element(t):
    match = re.fullmatch(r'(Option|Vec)<(.+)>', t)
    return match.groups() if match else None


def signatures(source, pattern):
    result = []
    for method in re.finditer(pattern, source):
        args = block(source, method.end() - 1)
        tail = source[method.end() + len(args):]
        returned = re.match(r'\)\s*->\s*([^\{]+)\{', tail)
        if not returned:
            raise ValueError(f'unsupported connection signature {method[2]}')
        output = short(returned[1])
        success, error = split(output[len('Result<'):-1]) if output.startswith('Result<') else (output, None)
        result.append({'name': method[2], 'async': bool(method[1]),
                       'parameters': fields(','.join(arg for arg in split(args) if arg != '&self')),
                       'success': success, 'error': error})
    return result


def methods(root):
    result = {}
    for owner, (path, adapter) in CLIENTS.items():
        source = clean((root / path).read_text())
        start = re.search(r'impl\s+' + owner + r'\s*\{', source)
        if not start:
            raise ValueError(f'missing canonical connection client {owner}')
        body = block(source, start.end() - 1)
        result[owner] = signatures(body, r'pub\s+(async\s+)?fn\s+(\w+)\s*\(')
        for method in result[owner]:
            method.update(source=path, adapter=adapter)
    return result


def functions(root):
    source = clean((root / next(iter(SOURCES))).read_text())
    # All public TCP convenience functions are owning endpoint constructors.
    # Their declarations, rather than an independent method list, define this API.
    return signatures(source, r'pub\s+(async\s+)?fn\s+(tcp_\w+)\s*\(')


def load(root):
    all_types = inventory(root, SOURCES)
    for item in all_types.values():
        for field in item.get('fields', []) + [f for v in item.get('variants', []) for f in v['fields']]:
            field['type'] = short(field['type'])
    found = {}
    def visit(t, direction):
        t = short(t)
        if t in PRIMITIVES | TRANSPORT | OPAQUE | {'&str', 'usize', 'NonZeroUsize', '[u8;16]', 'InterfaceId', 'SessionError'}:
            if direction == 'input' and (t in OPAQUE or t == 'SessionError'):
                raise ValueError(f'opaque native cause is output-only: {t}')
            return
        wrapped = element(t)
        if wrapped:
            visit(wrapped[1], direction)
            return
        if t not in all_types or all_types[t]['source'] not in SOURCES:
            raise ValueError(f'unknown canonical connection type {t}')
        item = found.setdefault(t, dict(copy.deepcopy(all_types[t]), directions=[]))
        if direction in item['directions']:
            return
        item['directions'].append(direction)
        for field in item.get('fields', []) + [f for v in item.get('variants', []) for f in v['fields']]:
            visit(field['type'], direction)
    visit('Options', 'input')
    visit('Profile', 'input')
    visit('Error', 'output')
    for method in [m for group in methods(root).values() for m in group] + functions(root):
        for field in method['parameters']:
            visit(field['type'], 'input')
        visit(method['success'], 'output')
        if method['error']:
            visit(method['error'], 'output')
    return found


def foreign(t):
    return 'Connection' + t


def generate(root):
    types = load(root)
    def typ(t):
        t = short(t)
        if t in TRANSPORT: return 'crate::transport::' + t
        if t in OPAQUE: return 'String'
        if t == 'SessionError': return 'crate::transport::BindingError'
        if t in {'ProfileId', '[u8;16]', 'InterfaceId'}: return 'Vec<u8>'
        if t == 'NonZeroUsize': return 'u32'
        if t == 'usize': return 'u64'
        if t == '&str': return 'String'
        wrapped = element(t)
        if wrapped: return wrapped[0] + '<' + typ(wrapped[1]) + '>'
        if t in types: return foreign(t)
        if t in PRIMITIVES: return t
        raise ValueError(f'unsupported connection representation {t}')
    def convert(t, value, direction):
        t = short(t)
        input = direction == 'input'
        if t in OPAQUE:
            if input: raise ValueError('native diagnostics cannot be lifted')
            return f'{value}.to_string()'
        if t == 'SessionError': return f'crate::facade::binding_error({value})'
        if t == 'ProfileId':
            return f'prns_host_runtime::connections::ProfileId::new(fixed16({value})?).map_err(ConnectionError::from)?' if input else f'{value}.bytes().to_vec()'
        if t == '[u8;16]': return f'fixed16({value})?' if input else f'{value}.to_vec()'
        if t == 'InterfaceId':
            if input: raise ValueError('attachment authority cannot be lifted by the profile API')
            return f'{value}.as_bytes().to_vec()'
        if t == 'usize': return f'usize::try_from({value}).map_err(|_| invalid(prns_host_runtime::connections::InputError::InvalidLimits))?' if input else f'{value} as u64'
        if t == 'NonZeroUsize':
            if not input: raise ValueError('construction budget is currently input-only')
            return f'core::num::NonZeroUsize::new(usize::try_from({value}).map_err(|_| invalid(prns_host_runtime::connections::InputError::InvalidLimits))?).ok_or_else(|| invalid(prns_host_runtime::connections::InputError::InvalidLimits))?'
        if t == '&str': return f'&{value}'
        if t in TRANSPORT:
            return f'{value}.try_into().map_err(|_| invalid(prns_host_runtime::connections::InputError::InvalidConfig))?' if input else f'{value}.into()'
        wrapped = element(t)
        if wrapped:
            kind, item = wrapped
            converted = convert(item, 'item', direction)
            if converted == 'item': return value
            if kind == 'Option':
                return f'{value}.map(|item| Ok::<_, ConnectionError>({converted})).transpose()?' if input else f'{value}.map(|item| {converted})'
            return f'{value}.into_iter().map(|item| Ok({converted})).collect::<Result<Vec<_>, ConnectionError>>()?' if input else f'{value}.into_iter().map(|item| {converted}).collect()'
        if t in types: return f'{value}.try_into()?' if input else f'{value}.into()'
        return value
    fingerprint = hashlib.sha256(json.dumps(types, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
    rust = ['// Generated from canonical native connection declarations. Do not edit.',
            '#![allow(clippy::needless_question_mark)]',
            'use crate::connections::{ConnectionProfileClient, ConnectionBindingClient};',
            f'pub const CONNECTION_SEMANTIC_FINGERPRINT: &str = "{fingerprint}";',
            'fn invalid(reason: prns_host_runtime::connections::InputError) -> ConnectionError { prns_host_runtime::connections::Error::Invalid(reason).into() }',
            'fn fixed16(value: Vec<u8>) -> Result<[u8;16], ConnectionError> { value.try_into().map_err(|_| invalid(prns_host_runtime::connections::InputError::InvalidId)) }']
    for name, item in types.items():
        if name == 'ProfileId': continue
        target = foreign(name)
        if item['kind'] == 'struct':
            if item['style'] != 'named' or any(not f['public'] for f in item['fields']):
                raise ValueError(f'unsupported private connection record {name}')
            rust += ['#[derive(Debug, uniffi::Record)]', f'pub struct {target} {{']
            rust += [f'pub {f["name"]}: {typ(f["type"])},' for f in item['fields']]
        elif item['kind'] == 'enum':
            error = name == 'Error'
            rust += ['#[derive(Debug, thiserror::Error, uniffi::Error)]' if error else '#[derive(Debug, uniffi::Enum)]', f'pub enum {target} {{']
            for variant in item['variants']:
                if error:
                    detail = ' '.join(f'{f["name"]}={{{f["name"]}:?}}' for f in variant['fields'])
                    rust += ['#[error(' + json.dumps(f'connection profiles: {variant["name"]} {detail}'.rstrip()) + ')]']
                payload = ' { ' + ', '.join(f'{f["name"]}: {typ(f["type"])}' for f in variant['fields']) + ' }' if variant['fields'] else ''
                rust += [variant['name'] + payload + ',']
        else: raise ValueError(f'unsupported connection declaration {name}')
        rust += ['}']
        native = item['namespace'] + '::' + name
        for direction in item['directions']:
            input = direction == 'input'
            source, destination = (target, native) if input else (native, target)
            rust += [f'impl TryFrom<{source}> for {destination} {{ type Error = ConnectionError; fn try_from(value: {source}) -> Result<Self, Self::Error> {{ Ok('] if input else [f'impl From<{source}> for {destination} {{ fn from(value: {source}) -> Self {{']
            if item['kind'] == 'struct':
                rust += ['Self { ' + ', '.join(f'{f["name"]}: {convert(f["type"], "value." + f["name"], direction)}' for f in item['fields']) + ' }']
            else:
                rust += ['match value {']
                for variant in item['variants']:
                    names = ', '.join(f['name'] for f in variant['fields'])
                    pattern = (' { ' + names + ' }') if (input and variant['fields']) or variant['style'] == 'named' else ('(' + names + ')') if variant['style'] == 'tuple' else ''
                    entries = [convert(f['type'], f['name'], direction) for f in variant['fields']]
                    if not input or variant['style'] == 'named':
                        payload = ' { ' + ', '.join(f['name'] if f['name'] == value else f'{f["name"]}: {value}' for f, value in zip(variant['fields'], entries)) + ' }' if entries else ''
                    else: payload = '(' + ', '.join(entries) + ')' if entries else ''
                    rust += [f'{source}::{variant["name"]}{pattern} => {destination}::{variant["name"]}{payload},']
                rust += ['}']
            rust += [') } }' if input else '} }']
    for owner, operations in methods(root).items():
        rust += ['#[uniffi::export]', f'impl {CLIENTS[owner][1]} {{']
        for operation in operations:
            if operation['error'] not in (None, 'Error'): raise ValueError('unknown connection client error')
            parameters = ', '.join(f'{f["name"]}: {typ(f["type"])}' for f in operation['parameters'])
            output = f'Result<{typ(operation["success"])}, ConnectionError>' if operation['error'] else typ(operation['success'])
            rust += [f'pub {"async " if operation["async"] else ""}fn {operation["name"]}(&self' + (', ' + parameters if parameters else '') + f') -> {output} {{']
            args = ', '.join(convert(f['type'], f['name'], 'input') for f in operation['parameters'])
            call = f'self.client.{operation["name"]}({args})' + ('.await' if operation['async'] else '')
            if operation['success'] == '()':
                rust += [f'{call}.map_err(ConnectionError::from)?;', 'Ok(())'] if operation['error'] else [f'{call};']
            else:
                rust += [f'let value = {call}.map_err(ConnectionError::from)?;', f'Ok({convert(operation["success"], "value", "output")})'] if operation['error'] else [f'let value = {call};', convert(operation['success'], 'value', 'output')]
            rust += ['}']
        rust += ['}']
    for function in functions(root):
        parameters = ', '.join(f'{f["name"]}: {typ(f["type"])}' for f in function['parameters'])
        args = ', '.join(convert(f['type'], f['name'], 'input') for f in function['parameters'])
        rust += ['#[uniffi::export]', f'pub fn connection_{function["name"]}({parameters}) -> Result<{typ(function["success"])}, ConnectionError> {{',
                 f'let value = prns_host_runtime::connections::{function["name"]}({args}).map_err(ConnectionError::from)?;',
                 f'Ok({convert(function["success"], "value", "output")})', '}']
    return format_rust('\n'.join(rust) + '\n'), types


def outputs(root):
    rust, types = generate(root)
    metadata = {'schemaVersion': 1, 'authoritativeSources': list(SOURCES),
                'types': list(types.values()), 'clients': methods(root), 'functions': functions(root),
                'representations': {'ProfileId': 'checked native new([u8;16]) via bytes', 'InterfaceId': 'output bytes',
                                    'opaqueNativeCauses': sorted(OPAQUE), 'transportTypes': sorted(TRANSPORT)},
                'feature': 'connections', 'defaultEnabled': False}
    return {root / 'prns-host/bindings/uniffi/src/connections.generated.rs': rust,
            root / 'prns-host/schema/connections-v1.generated.json': json.dumps(metadata, indent=2) + '\n'}
