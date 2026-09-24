"""Derive the foreign Remote Control vocabulary from authoritative Rust declarations.

The narrow parser accepts only closed structs/enums and known bounded wrappers,
including the core iterable_enum and desired_state declarations. Unknown syntax
fails generation. No app DTO or operation inventory is maintained here.
"""
import hashlib
import json
import re
import copy
from pathlib import Path
from .formatting import format_rust


def split(text, delimiter=','):
    result, start, stack = [], 0, []
    for i, char in enumerate(text):
        if char in '({[<': stack.append(char)
        elif char in ')}]>' and stack and stack[-1] == {')':'(', '}':'{', ']':'[', '>':'<'}[char]: stack.pop()
        elif char == delimiter and not stack:
            if text[start:i].strip(): result.append(text[start:i].strip())
            start = i + 1
    if text[start:].strip(): result.append(text[start:].strip())
    return result


def block(source, start):
    end, depth = start, 0
    left, right = source[start], {'{': '}', '(': ')', '<': '>'}[source[start]]
    for end in range(start, len(source)):
        if source[end] == left: depth += 1
        elif source[end] == right:
            depth -= 1
            if not depth: return source[start + 1:end]
    raise ValueError('unclosed Rust declaration')


def clean(source):
    source = re.sub(r'/\*.*?\*/', '', source, flags=re.S)
    source = re.sub(r'//[^\n]*', '', source)
    return re.sub(r'#\[[^\]]*\]', '', source)


def fields(body):
    result = []
    for value in split(body):
        match = re.fullmatch(r'(pub(?:\([^)]*\))?\s+)?(\w+)\s*:\s*(.+)', value, flags=re.S)
        if not match: raise ValueError(f'unsupported Rust field: {value}')
        result.append({'name': match[2], 'type': re.sub(r'\s+', '', match[3]), 'public': match[1] == 'pub '})
    return result


def inventory(root):
    result = {}
    extra = {'prns-core/src/wire/header.rs':'prns_core::wire',
             'prns-core/src/routing/announce/mod.rs':'prns_core::routing::announce',
             'prns-core/src/identity/held/core.rs':'prns_core::identity::held',
             'prns-core/src/routing/upstream_app_destinations/core.rs':'prns_core::routing::upstream_app_destinations',
             'prns-core/src/routing/delivery/send_plain.rs':'prns_core::routing::delivery::send_plain',
             'prns-core/src/storage/core.rs':'prns_core::storage'}
    paths = list((root / 'prns-core/src/remote_control').rglob('*.rs'))
    paths += list((root / 'prns-core/src/interfaces').rglob('*.rs'))
    paths += [root / 'prns-core/src/capabilities/power.rs']
    paths += list((root / 'prns-core/src/engine').rglob('*.rs'))
    paths += [root/'prns-core/src/units.rs',root/'prns-core/src/routing/links/request.rs']
    paths += list((root / 'prns-runtime/core/src/runtime').glob('*.rs'))
    paths += list((root / 'prns-core/src/routing/links/resources/table').glob('*.rs'))
    paths += [root / 'prns-core/src/routing/links/resources/mod.rs', root / 'prns-core/src/routing/links/resources/table/mod.rs', root / 'prns-core/src/routing/links/establish/mod.rs', root / 'prns-host/impls/native/src/lib.rs', root / 'prns-host/impls/native/src/remote_control.rs']
    paths += [root/'prns-host/impls/native/src/remote_control_service.rs']
    paths += [root/path for path in extra]
    for path in paths:
        if 'tests' in path.parts or path.name == 'tests.rs': continue
        source = clean(path.read_text())
        namespace = ('native_service' if path.name=='remote_control_service.rs' else 'native' if 'native' in path.parts else 'request' if path.name=='request.rs' and 'links' in path.parts else 'units' if path.name=='units.rs' else 'runtime' if 'prns-runtime' in path.parts else 'engine' if 'engine' in path.parts else 'resource_table' if 'table' in path.parts else 'resources' if 'resources' in path.parts else 'establish' if 'establish' in path.parts else 'power' if path.name == 'power.rs' else 'rc' if 'remote_control' in path.parts else 'iface')
        if str(path.relative_to(root)) in extra: namespace=extra[str(path.relative_to(root))]
        for match in re.finditer(r'pub (struct|enum) (\w+)(?:<([^>{}]*)>)?\s*([\{\(])', source):
            kind, name, parameters, opener = match.groups()
            body = block(source, match.end() - 1)
            item = {'name': name, 'kind': kind, 'namespace': namespace, 'source': str(path.relative_to(root))}
            if parameters: item['parameters']=split(parameters)
            if kind == 'enum':
                variants = []
                for value in split(body):
                    variant = re.fullmatch(r'(\w+)(?:\s*=.*|\s*(\{.*\}|\(.*\)))?', value, flags=re.S)
                    if not variant: raise ValueError(f'unsupported variant {name}: {value}')
                    payload = variant[2]
                    variants.append({'name': variant[1], 'style': 'unit' if not payload else ('named' if payload[0] == '{' else 'tuple'),
                        'fields': [] if not payload else (fields(payload[1:-1]) if payload[0] == '{' else [{'name': 'value' if len(split(payload[1:-1])) == 1 else f'value{i}', 'type': re.sub(r'\s+', '', t), 'public': True} for i, t in enumerate(split(payload[1:-1]))])})
                item['variants'] = variants
            else:
                item['style'] = 'named' if opener == '{' else 'tuple'
                item['fields'] = fields(body) if opener == '{' else [{'name':'value','type':body.removeprefix('pub ').strip(),'public':body.startswith('pub ')}]
                for field in item['fields']:
                    field['borrowed']=bool(re.search(r'fn '+field['name']+r'\(\s*&self\s*\)\s*->\s*&',source))
            if name in result and result[name] != item: continue
            result[name] = item
        for match in re.finditer(r'desired_state!\((\w+)\s*\{', source):
            body = block(source, match.end()-1)
            result[match[1]] = {'name':match[1], 'kind':'enum','namespace':namespace,'source':str(path.relative_to(root)),
                'variants':[{'name':v.split('=')[0].strip(),'style':'unit','fields':[]} for v in split(body)]}
        for match in re.finditer(r'pub type (\w+)\s*=\s*([^;]+);',source):
            result[match[1]]={'name':match[1],'kind':'alias','namespace':namespace,'source':str(path.relative_to(root)),'alias':re.sub(r'\s+','',match[2])}
    return result


def operations(root):
    result=[]
    for file,trait in [('remote_control_pairing.rs','RemoteControlPairingControl'),('remote_control_target_accesses.rs','RemoteControlTargetAccessControl'),('command.rs','PrnsNodeApi')]:
        source=clean((root/'prns-runtime/core/src/runtime'/file).read_text())
        start=source.index('pub trait '+trait)
        body=block(source,source.index('{',start))
        for method in re.finditer(r'fn (\w+)\s*\(',body):
            if trait=='PrnsNodeApi' and not method[1].endswith('_remote_control_pairing'): continue
            args=block(body,method.end()-1)
            tail=body[method.end()+len(args):]
            returned=tail.index('Result<')+len('Result')
            success,error=[re.sub(r'(?:(?:crate|super|table|engine)::)+','',t) for t in split(block(tail,returned))]
            result.append({'name':method[1], 'trait':trait,
                'parameters':fields(','.join(arg for arg in split(args) if arg != '&self')),
                'success':success,'error':error})
    return result

# Semantic views of bounded private storage. The declarations above remain the
# authoritative public field/variant inventory; these are constructor/accessor
# mappings only, analogous to the host DestinationName constructor mapping.
CUSTOM = {
    'RemoteControlRequestSet': [('kinds','Vec<RemoteControlRequestKind>')],
    'RemoteControlWifiStation': [('ssid','String'),('password','String')],
    'RemoteControlLoRaProfile': [('value','String')],
    'RemoteControlBuildVersion': [('value','String')],
    'RemoteControlInterfaceGroup': [('value','String')],
    'RemoteControlDiscoveryGroups': [('groups','Vec<String>')],
    'RemoteControlControllerIdentity': [('public_keys','Bytes')],
    'RemoteControlInterfaceCursor': [('value','InterfaceId')],
    'RemoteControlPeerCursor': [('value','InterfaceId')],
    'RemoteControlControllerCursor': [('value','IdentityHash')],
    'RemoteControlWifiCredentialRevision': [('value','u32')],
    'RemoteControlWifiConfirmationRemaining': [('value','u8')],
    'BatteryPercent': [('value','u8')],
    'RssiDbm': [('value','i16')],
    'SnrQuarterDb': [('value','i16')],
    'SignalQualityTenthsPercent': [('value','u16')],
    'InterfaceId': [('value','Bytes')],
    'IdentityHash': [('value','Bytes')],
    'DestinationHash': [('value','Bytes')],
    'PacketHash': [('value','Bytes')],
    'RequestId': [('value','Bytes')],
    'LinkId': [('value','Bytes')],
    'RemoteControlPairingInvitationCode': [('value','u32')],
    'RemoteControlPairingAttemptId': [('value','Bytes')],
    'RemoteControlPairingTranscriptDigest': [('value','Bytes')],
    'RemoteControlTargetIdentity': [('public_keys','Bytes')],
    'RemoteControlTargetInventory': [('targets','Vec<IdentityHash>')],
    'RttMillis': [('value','u64')],
    'RemoteControlPairingAttemptTimeout': [('value','u64')],
    'RemoteControlPairingExpiresAfter': [('value','u64')],
    'RemoteControlPairingPublicAppDataBytes': [('value','Bytes')],
}


def inner(t):
    match = re.fullmatch(r'(Option|Vec|heapless::Vec)<(.+)>', t)
    if match: return split(match[2])[0]
    return None


def load(root):
    all_types = inventory(root)
    for item in all_types.values():
        for field in item.get('fields',[]) + [f for v in item.get('variants',[]) for f in v['fields']]:
            field['type']=re.sub(r'(?:(?:crate|super|table|engine|storage)::)+', '', field['type'])
    for name,namespace in [('InterfaceId','iface'),('IdentityHash','identity'),('DestinationHash','wire'),('PacketHash','dedup'),('LinkId','link')]:
        all_types[name] = {'name':name, 'kind':'struct', 'namespace':namespace, 'source':'prns-core/src', 'style':'named'}
    for name, field_list in CUSTOM.items():
        if name not in all_types: raise ValueError(f'missing authoritative type {name}')
        all_types[name]['fields'] = [{'name':n,'type':t,'public':False} for n,t in field_list]
        all_types[name]['custom'] = True
    found = {}
    def visit(name, direction):
        if name in ('u8','u16','u32','u64','i8','i16','i32','i64','usize','bool','String','Bytes','IdentityConfig') or name.startswith('heapless::String<'): return
        if inner(name): return visit(inner(name), direction)
        if name in all_types and all_types[name]['kind']=='alias':
            alias=all_types[name]
            visit(alias['alias'],direction)
            all_types[name]=dict(all_types[alias['alias']],name=name,core=core(alias))
        generic=re.fullmatch(r'(\w+)<(.+)>',name)
        if generic and name not in all_types:
            base,args=generic[1],split(generic[2])
            item=copy.deepcopy(all_types[base])
            item['name']=name
            item['core']=core(all_types[base])+'<'+','.join(core(all_types[arg]) for arg in args)+'>'
            for field in item.get('fields',[]) + [f for v in item.get('variants',[]) for f in v['fields']]:
                for param,arg in zip(item.pop('parameters',all_types[base]['parameters']),args):
                    field['type']=re.sub(r'\b'+param+r'\b',arg,field['type'])
            all_types[name]=item
        if name not in all_types: raise ValueError(f'unknown Rust protocol type {name}')
        item = found.setdefault(name, dict(all_types[name], directions=[]))
        if direction in item['directions']: return
        item['directions'].append(direction)
        for field in item.get('fields',[]) + [f for v in item.get('variants',[]) for f in v['fields']]: visit(field['type'], direction)
    visit('RemoteControlRequest', 'input')
    visit('RemoteControlResponse', 'output')
    visit('RemoteControlExchangeSettlement', 'output')
    visit('NativeRemoteControlConfig','input')
    visit('NativeRemoteControlEvent','output')
    for operation in operations(root):
        for field in operation['parameters']: visit(field['type'],'input')
        visit(operation['success'],'output')
        visit(operation['error'],'output')
        name=''.join(part.title() for part in operation['name'].split('_'))+'Settlement'
        found[name]={'name':name,'kind':'enum','namespace':'binding','binding':True,'directions':['output'],
            'variants':[{'name':'Completed','style':'named','fields':[{'name':'value','type':operation['success'],'public':True}]},
                        {'name':'Failed','style':'named','fields':[{'name':'failure','type':operation['error'],'public':True}]}]}
    return found


def foreign(name):
    generic=re.fullmatch(r'(\w+)<(.+)>',name)
    if generic: return foreign(generic[1])+''.join(foreign(arg) for arg in split(generic[2]))
    return name if name.startswith('RemoteControl') else 'RemoteControl' + name


def core(item):
    if 'core' in item: return item['core']
    if item['namespace'].startswith('prns_core::'): return item['namespace']+'::'+item['name']
    if item['namespace']=='native_service': return 'prns_host_native::remote_control_service::'+item['name']
    namespace = {'rc':'prns_core::remote_control','iface':'prns_core::interfaces','identity':'prns_core::identity','power':'prns_core::capabilities::power', 'runtime':'personal_rns::runtime','engine':'prns_core::engine','resources':'prns_core::routing::links::resources','resource_table':'prns_core::routing::links::resources::table','establish':'prns_core::routing::links::establish','native':'prns_host_native::remote_control','wire':'prns_core::wire','link':'prns_core::routing::links','units':'prns_core::units','request':'prns_core::routing::links::request','dedup':'prns_core::routing::dedup'}[item['namespace']]
    if item['name']=='NativeSubmitError': namespace='prns_host_native'
    return namespace + '::' + item['name']


def generate(root):
    types = load(root)
    def typ(t):
        if t=='IdentityConfig': return 'crate::transport::IdentityConfig'
        if t == 'usize': return 'u64'
        if t == 'Bytes': return 'Vec<u8>'
        if t.startswith('heapless::String<'): return 'String'
        if inner(t): return ('Option' if t.startswith('Option') else 'Vec') + '<' + typ(inner(t)) + '>'
        return foreign(t) if t in types else t
    def convert(t, expression, direction):
        if t=='IdentityConfig': return f'{expression}.try_into()?'
        if t == 'usize': return f'usize::try_from({expression}).map_err(|_| invalid("usize"))?' if direction=='input' else f'{expression} as u64'
        elem = inner(t)
        if elem:
            converted = convert(elem,'item',direction)
            if converted == 'item' and not t.startswith('heapless::Vec<'):
                return expression
            if t.startswith('Option'):
                return f'{expression}.map(|item| Ok({converted})).transpose()?' if direction=='input' else f'{expression}.map(|item| {converted})'
            if direction=='input':
                return f'{expression}.into_iter().map(|item| Ok({converted})).collect::<Result<Vec<_>, BindingError>>()?'
            # Borrow bounded storage until all field accessors have been read;
            # collect the converted values without an intermediate Vec clone.
            iterator = f'{expression}.iter().cloned()' if t.startswith('heapless::Vec<') else f'{expression}.into_iter()'
            return f'{iterator}.collect()' if converted == 'item' else f'{iterator}.map(|item| {converted}).collect()'
        if t.startswith('heapless::String<'): return f'{expression}.to_string()'
        if t in types: return f'{expression}.try_into()?' if direction=='input' else f'{expression}.into()'
        return expression
    fingerprint=hashlib.sha256(json.dumps(types,sort_keys=True,separators=(',',':')).encode()).hexdigest()
    rust = ['// Generated from public prns-core Remote Control declarations. Do not edit.',
            '// Uniform fallible converters retain ? for nested validation and error conversion.',
            '#![allow(clippy::needless_question_mark)]',
            f'pub const REMOTE_CONTROL_SEMANTIC_FINGERPRINT: &str = "{fingerprint}";', 'use crate::transport::BindingError;', '''pub struct RemoteControlSecretText(zeroize::Zeroizing<String>);
uniffi::custom_type!(RemoteControlSecretText, String, {
    lower: |value| value.0.to_string(),
    try_lift: |value| Ok(RemoteControlSecretText(zeroize::Zeroizing::new(value))),
});
pub struct RemoteControlSecretCode(zeroize::Zeroizing<u32>);
uniffi::custom_type!(RemoteControlSecretCode, u32, {
    lower: |value| *value.0,
    try_lift: |value| Ok(RemoteControlSecretCode(zeroize::Zeroizing::new(value))),
});''', 'fn invalid(name: &str) -> BindingError { BindingError::InvalidInput { field:name.into(), detail:"invalid protocol value".into() } }']
    for name,item in types.items():
        target = foreign(name)
        derive = 'uniffi::Enum' if item['kind']=='enum' else 'uniffi::Record'
        rust += [f'#[derive({derive})]']
        if item['kind']=='struct':
            rust += [f'pub struct {target} {{'] + [f'    pub {f["name"]}: {"RemoteControlSecretText" if name == "RemoteControlWifiStation" and f["name"] == "password" else "RemoteControlSecretCode" if name == "RemoteControlPairingInvitationCode" else typ(f["type"])},' for f in item['fields']] + ['}']
        else:
            rust += [f'pub enum {target} {{']
            for v in item['variants']:
                fs = ', '.join(f'{f["name"]}: {typ(f["type"])}' for f in v['fields'])
                rust += [f'    {v["name"]}' + (' { '+fs+' }' if fs else '') + ',']
            rust += ['}']
        for direction in ([] if item.get('binding') else item['directions']):
            input = direction=='input'
            src,dst = (target,core(item)) if input else (core(item),target)
            rust += [f'impl TryFrom<{src}> for {dst} {{ type Error = BindingError; fn try_from(value: {src}) -> Result<Self, Self::Error> {{ Ok(' if input else f'impl From<{src}> for {dst} {{ fn from(value: {src}) -> Self {{']
            if item['kind']=='enum':
                rust += ['match value {']
                for v in item['variants']:
                    names = [f['name'] for f in v['fields']]
                    pattern = '' if not names else (' { '+', '.join(names)+' }' if input or v['style']=='named' else '('+', '.join(names)+')')
                    converted = [convert(f['type'],f['name'],direction) for f in v['fields']]
                    payload = '' if not names else (' { '+', '.join(n if n == c else n+': '+c for n,c in zip(names,converted))+' }' if not input or v['style']=='named' else '('+', '.join(converted)+')')
                    rust += [f'{src.replace("<", "::<")}::{v["name"]}{pattern} => {dst.replace("<", "::<")}::{v["name"]}{payload},']
                rust += ['}']
            elif item.get('custom'):
                rust += [custom_conversion(name,core(item),input)]
            elif item.get('style')=='tuple':
                f=item['fields'][0]
                rust += [f'{core(item)}({convert(f["type"],"value.value",direction)})' if input else f'Self {{ value: {convert(f["type"],"value.0",direction)} }}']
            elif input and any(not f['public'] for f in item['fields']):
                constructor='from_destination_hash' if name=='RemoteControlPairingEndpoint' else 'from_requests' if name=='RemoteControlCapabilities' else 'new'
                expression=core(item)+'::'+constructor+'('+', '.join(convert(f['type'],'value.'+f['name'],direction) for f in item['fields'])+')'
                if name in ('RemoteControlTargetAccess','RemoteControlControllerGrant','RemoteControlCapabilities','RemoteControlPairingPermissions'):
                    expression += f'.map_err(|_| invalid("{name}"))?'
                rust += [expression]
            else:
                fs = []
                for f in item['fields']:
                    expression = 'value.' + f['name']
                    if not input and not f['public']:
                        expression += '()'
                        if f.get('borrowed') and not f['type'].startswith('heapless::Vec<'):
                            expression = ('prns_core::remote_control::RemoteControlTargetIdentity::new(*' + expression + '.public_keys())') if f['type']=='RemoteControlTargetIdentity' else '(*' + expression + ')'
                    fs.append(f'{f["name"]}: {convert(f["type"],expression,direction)}')
                rust += ['Self { '+', '.join(fs)+' }']
            rust += [') } }' if input else '} }']
    rust += ['use crate::facade::HostClientHandle;', '#[uniffi::export]', 'impl HostClientHandle {']
    for operation in operations(root):
        name=operation['name'];settlement=foreign(''.join(part.title() for part in name.split('_'))+'Settlement')
        parameters=', '.join(f'{f["name"]}: {typ(f["type"])}' for f in operation['parameters'])
        rust += [f'pub async fn {name}(&self'+(', '+parameters if parameters else '')+f') -> Result<{settlement}, BindingError> {{']
        for f in operation['parameters']:
            rust += [f'let {f["name"]}: {core(types[f["type"]])} = {convert(f["type"],f["name"],"input")};']
        args=''.join(', '+f['name'] for f in operation['parameters'])
        rust += [f'let result = self.client.protocol_operation(move |handle| async move {{ personal_rns::runtime::{operation["trait"]}::{name}(&handle{args}).await }}).await.map_err(crate::facade::binding_error)?;',
            f'Ok(match result {{ Ok(value) => {settlement}::Completed {{ value: value.into() }}, Err(failure) => {settlement}::Failed {{ failure: failure.into() }} }})', '}']
    rust += ['}']
    return format_rust('\n'.join(rust)+'\n'), types


def custom_conversion(name, path, input):
    if name in ('InterfaceId','IdentityHash','DestinationHash','PacketHash','LinkId','RequestId'):
        return f'{path}::new(value.value.try_into().map_err(|_| invalid("{name}"))?)' if input else 'Self { value: value.as_bytes().to_vec() }'
    if name in ('RemoteControlControllerIdentity','RemoteControlTargetIdentity'):
        return f'{path}::new(prns_core::identity::PublicIdentityMaterial::from_slice(&value.public_keys).map_err(|_| invalid("publicKeys"))?.public_keys())' if input else 'Self { public_keys: value.public_keys().public_key_bytes().to_vec() }'
    if name=='RemoteControlRequestSet':
        return '{ let mut set = '+path+'::empty(); for kind in value.kinds { set.insert(kind.try_into()?); } set }' if input else 'Self { kinds: value.iter().map(Into::into).collect() }'
    if name=='RemoteControlWifiStation':
        if not input: raise ValueError('secret output not permitted')
        return f'{path}::parse(&value.ssid, &value.password.0).map_err(|_| invalid("WifiStation"))?'
    if name in ('RemoteControlLoRaProfile','RemoteControlBuildVersion','RemoteControlInterfaceGroup'):
        constructor = 'from_text' if name.endswith('BuildVersion') else 'parse'
        return f'{path}::{constructor}(&value.value).ok_or_else(|| invalid("{name}"))?' if input else 'Self { value: value.as_str().unwrap_or("").to_owned() }'
    if name=='RemoteControlDiscoveryGroups':
        return '{ let groups = value.groups.into_iter().map(|text| prns_core::interfaces::DiscoveryGroupId::parse(&text).map_err(|_| invalid("DiscoveryGroup"))).collect::<Result<Vec<_>, _>>()?; '+path+'::new(prns_core::interfaces::DiscoveryGroupSet::try_from_slice(&groups).map_err(|_| invalid("DiscoveryGroups"))?) }' if input else 'Self { groups: value.groups().iter().map(|group| group.as_str().to_owned()).collect() }'
    if name=='RemoteControlTargetInventory':
        return 'Self { targets: value.targets().iter().map(|target| target.identity_hash().into()).collect() }'
    if name=='RemoteControlPairingAttemptId':
        return f'{path}::from_transcript_digest_bytes(value.value.try_into().map_err(|_| invalid("attemptId"))?)' if input else 'Self { value: value.transcript().as_bytes().to_vec() }'
    if name=='RemoteControlPairingTranscriptDigest':
        return 'Self { value: value.as_bytes().to_vec() }'
    if name in ('RemoteControlPairingAttemptTimeout','RemoteControlPairingExpiresAfter'):
        return f'{path}::try_from(prns_core::units::DurationMillis(value.value)).map_err(|_| invalid("{name}"))?' if input else 'Self { value: value.duration().0 }'
    if name=='RemoteControlPairingPublicAppDataBytes':
        return f'{path}::try_from(value.value.as_slice()).map_err(|_| invalid("{name}"))?' if input else 'Self { value: value.as_bytes().to_vec() }'
    if name=='RemoteControlPairingInvitationCode':
        return f'{path}::from_value(*value.value.0)' if input else 'Self { value: RemoteControlSecretCode(zeroize::Zeroizing::new(value.value())) }'
    if name.endswith('Cursor'):
        getter = 'identity' if 'Controller' in name else 'id'
        return f'{path}::after(value.value.try_into()?)' if input else f'Self {{ value: value.{getter}().into() }}'
    getter = {'RemoteControlWifiCredentialRevision':'get','RemoteControlWifiConfirmationRemaining':'seconds','BatteryPercent':'get','RssiDbm':'get','SnrQuarterDb':'quarters','SignalQualityTenthsPercent':'tenths_percent','RttMillis':'millis'}[name]
    if input: return f'{path}::new(value.value).ok_or_else(|| invalid("{name}"))?'
    return f'Self {{ value: value.{getter}() }}'


def camel(name):
    parts = name.split('_')
    return parts[0] + ''.join(part.title() for part in parts[1:])


def typescript(types,root):
    def typ(t):
        if t=='IdentityConfig': return 'IdentityConfig'
        if t=='Bytes' or t=='Vec<u8>' or t.startswith('heapless::Vec<u8,'): return 'Uint8Array'
        if t=='String' or t.startswith('heapless::String<'): return 'string'
        if t=='bool': return 'boolean'
        if t in ('u64','i64','usize'): return 'bigint'
        if re.fullmatch('[iu](8|16|32)',t): return 'number'
        if inner(t): return f'{typ(inner(t))} | undefined' if t.startswith('Option') else f'ReadonlyArray<{typ(inner(t))}>'
        return foreign(t)
    def record(fs):
        return '{ '+ '; '.join(f'readonly {camel(f["name"])}'+('?' if f['type'].startswith('Option<') else '')+f': {typ(inner(f["type"]) if f["type"].startswith("Option<") else f["type"])}' for f in fs) + ' }'
    fingerprint=hashlib.sha256(json.dumps(types,sort_keys=True,separators=(',',':')).encode()).hexdigest()
    lines=['// Generated from public prns-core Remote Control declarations. Do not edit.', 'import type { IdentityConfig } from "./contract.generated.js";', f'export const REMOTE_CONTROL_SEMANTIC_FINGERPRINT = "{fingerprint}";']
    for name,item in types.items():
        if item['kind']=='struct': lines += [f'export type {foreign(name)} = {record(item["fields"])};']
        else:
            lines += [f'export type {foreign(name)} =']
            lines += ['  | { readonly tag: '+json.dumps(v['name'])+ ('; readonly data: '+record(v['fields']) if v['fields'] else '')+' }' for v in item['variants']]
            lines[-1] += ';'
    adapter=['// Generated from public prns-core Remote Control declarations. Do not edit.',
        'import type * as C from "personal-rns/remote-control";',
        'import * as N from "./prns_host_uniffi";',
        'import { lowerIdentityConfig } from "./host-adapter.generated";',
        'function unexpected(value: never): never { throw new TypeError(`unknown Remote Control case: ${String(value)}`); }']
    def conversion(t,expr,direction):
        if t=='IdentityConfig': return f'lowerIdentityConfig({expr})'
        if t=='Bytes' or t=='Vec<u8>' or t.startswith('heapless::Vec<u8,'): return expr
        if inner(t):
            if t.startswith('Option'):
                return f'({expr} === undefined ? undefined : {conversion(inner(t),expr,direction)})'
            return f'{expr}.map(item => {conversion(inner(t),"item",direction)})'
        if t in types: return f'{direction}{foreign(t)}({expr})'
        return expr
    def mapping(fs,prefix,direction):
        values=[]
        for f in fs:
            n=camel(f['name']);e=f'{prefix}.{n}';t=f['type']
            if t.startswith('Option'):
                values += [f'...({e} === undefined ? {{}} : {{ {n}: {conversion(inner(t),e,direction)} }})']
            else: values += [n+': '+conversion(t,e,direction)]
        return '{ '+', '.join(values)+' }'
    for name,item in types.items():
        target=foreign(name)
        flat=item['kind']=='enum' and all(not v['fields'] for v in item['variants'])
        for direction in item['directions']:
            lower=direction=='input';fn='lower' if lower else 'lift';src,dst=('C','N') if lower else ('N','C')
            adapter += [f'export function {fn}{target}(value: {src}.{target}): {dst}.{target} {{']
            if item['kind']=='struct': adapter += ['  return '+mapping(item['fields'],'value',fn)+';']
            else:
                adapter += ['  switch (value'+('' if flat and not lower else '.tag')+') {']
                for v in item['variants']:
                    case=json.dumps(v['name']) if lower else f'N.{target}{"" if flat else "_Tags"}.{v["name"]}'
                    if lower:
                        value=f'N.{target}.{v["name"]}' if flat else f'N.{target}.{v["name"]}.new('+(mapping(v['fields'],'value.data',fn) if v['fields'] else '')+')'
                    else: value='{ tag: '+json.dumps(v['name'])+(', data: '+mapping(v['fields'],'value.inner',fn) if v['fields'] else '')+' }'
                    adapter += [f'    case {case}: return {value};']
                adapter += ['    default: return unexpected(value'+('.tag' if len(item['variants'])==1 and not flat else '')+');','  }']
            adapter += ['}']
    adapter += ['export function bindRemoteControlOperations(host: () => N.HostClientHandle) {', '  return {']
    for operation in operations(root):
        name=operation['name'];settlement=foreign(''.join(part.title() for part in name.split('_'))+'Settlement')
        parameters=', '.join(f'{camel(f["name"])}: C.{foreign(f["type"])}' for f in operation['parameters'])
        args=', '.join(conversion(f['type'],camel(f['name']),'lower') for f in operation['parameters'])
        adapter += [f'    async {camel(name)}('+ (parameters+', ' if parameters else '')+ f'options?: {{signal: AbortSignal}}): Promise<C.{settlement}> {{',
                    f'      return lift{settlement}(await host().{camel(name)}('+ (args+', ' if args else '')+'options));', '    },']
    adapter += ['  };','}','export type RemoteControlOperations = ReturnType<typeof bindRemoteControlOperations>;']
    return '\n'.join(lines)+'\n', '\n'.join(adapter)+'\n'


def outputs(root):
    rust,types=generate(root)
    ts,adapter=typescript(types,root)
    metadata={'schemaVersion':1,'authoritativeRoots':['prns_core::remote_control::RemoteControlRequest','prns_core::remote_control::RemoteControlResponse'],
        'types':list(types.values()),'targets':{'native':'supported','uniffi':'supported','c':'unsupported','napi':'unsupported','cooperative':'unsupported'},
        'unsupportedReason':'This typed protocol extension currently requires the native host task lane. Existing target APIs remain unchanged.'}
    return {
        root/'prns-host/bindings/uniffi/src/remote_control.generated.rs':rust,
        root/'prns-js/src/remote-control.generated.ts':ts,
        root/'prns-host/bindings/uniffi/typescript/remote-control-adapter.generated.ts':adapter,
        root/'prns-host/schema/remote-control-v1.generated.json':json.dumps(metadata,indent=2)+'\n',
    }
