import unittest
from pathlib import Path
from unittest.mock import patch
from tools.repo.host_contract import connections

ROOT = Path(__file__).resolve().parents[2]
MODEL = ROOT / 'prns-host/impls/runtime/src/connections/model.rs'
CLIENT = ROOT / 'prns-host/impls/runtime/src/connections/mod.rs'


class ConnectionProjectionTests(unittest.TestCase):
    def altered(self, path, before, after):
        original = Path.read_text
        def read(current, *args, **kwargs):
            value = original(current, *args, **kwargs)
            if current == path:
                self.assertIn(before, value)
                return value.replace(before, after, 1)
            return value
        return patch.object(Path, 'read_text', read)

    def test_fields_and_variants_follow_the_owning_declarations(self):
        with self.altered(MODEL, 'pub struct Profile {', 'pub struct Profile { pub priority: Option<u32>,'):
            rust, types = connections.generate(ROOT)
        self.assertEqual(types['Profile']['fields'][0]['name'], 'priority')
        self.assertIn('pub priority: Option<u32>', rust)
        self.assertEqual(rust.count('priority: value.priority'), 2)
        with self.altered(MODEL, 'pub enum InputError {', 'pub enum InputError { FutureValidation,'):
            rust, _ = connections.generate(ROOT)
        self.assertIn('InputError::FutureValidation', rust)

    def test_ordinary_client_methods_are_not_a_second_method_inventory(self):
        with self.altered(CLIENT, 'impl Client {', 'impl Client { pub async fn another_read(&self) -> Result<Vec<Snapshot>, Error> { todo!() }'):
            rust, _ = connections.generate(ROOT)
        self.assertIn('pub async fn another_read', rust)
        self.assertIn('.another_read()', rust)
        self.assertIn('Result<Vec<ConnectionSnapshot>, ConnectionError>', rust)

    def test_unknown_field_syntax_fails_closed(self):
        with self.altered(MODEL, 'pub struct Profile {', 'pub struct Profile { pub unsupported: NotYetRepresentable,'):
            with self.assertRaisesRegex(ValueError, 'NotYetRepresentable'):
                connections.generate(ROOT)

    def test_existing_transport_and_checked_identifiers_are_reused(self):
        rust, types = connections.generate(ROOT)
        self.assertIn('pub config: crate::transport::InterfaceConfig', rust)
        self.assertNotIn('pub enum ConnectionInterfaceConfig', rust)
        self.assertIn('ProfileId::new(fixed16(', rust)
        self.assertNotIn('ProfileId(value', rust)
        self.assertIn('usize::try_from(value.cache_bytes)', rust)
        self.assertEqual(types['ProfileId']['source'], str(MODEL.relative_to(ROOT)))

    def test_diagnostics_remain_output_only_and_conflict_keeps_snapshot(self):
        rust, types = connections.generate(ROOT)
        self.assertEqual(types['Error']['directions'], ['output'])
        self.assertIn('Conflict { value: ConnectionSnapshot }', rust)
        self.assertIn('crate::facade::binding_error', rust)
        self.assertNotIn('impl TryFrom<ConnectionError>', rust)
        self.assertIn('ConnectionAttachmentError::RetirementPending', rust)

    def test_endpoint_convenience_functions_are_discovered(self):
        functions = connections.functions(ROOT)
        self.assertIn('tcp_client_config_from_target', {f['name'] for f in functions})
        rust, _ = connections.generate(ROOT)
        self.assertIn('pub fn connection_tcp_client_config_from_target', rust)
        self.assertIn('prns_host_runtime::connections::tcp_client_config_from_target', rust)


if __name__ == '__main__':
    unittest.main()
