"""Assembly must not become a second public domain method/type inventory."""
import unittest
from unittest.mock import patch

import generate


class NamespaceAssembly(unittest.TestCase):
    def sources(self):
        ts = generate.BINDINGS / 'typescript'
        return {
            ts / 'prns_chat_app.ts': 'export function appApiVersion() { return 1; }\n',
            ts / 'prns_chat_app-ffi.ts': 'app ABI checksums\n',
            ts / 'prns_host_uniffi.ts': 'host implementation\n',
            ts / 'prns_host_uniffi-ffi.ts': 'host ABI checksums\n',
            ts / 'prns_new_domain.ts': 'arbitrary future domain; new methods stay here\n',
            ts / 'prns_new_domain-ffi.ts': 'future ABI checksums\n',
            generate.BINDINGS / 'swift/Future.swift': 'future Swift methods\n',
            generate.BINDINGS / 'kotlin/future/Future.kt': 'future Kotlin methods\n',
        }

    def test_new_namespace_is_preserved_and_initialized_without_method_inventory(self):
        sources = self.sources()
        result = generate.assemble(sources, 7)
        for path, content in sources.items():
            if path.name not in {'prns_host_uniffi.ts', 'prns_host_uniffi-ffi.ts'}:
                self.assertEqual(result[path], content)
        index = result[generate.BINDINGS / 'index.ts']
        native = result[generate.BINDINGS / 'index.native.ts']
        self.assertIn('export * as prns_new_domain', index)
        self.assertIn('prns_new_domain.initialize();', native)
        self.assertIn('appApiVersion() !== 7', native)
        self.assertNotIn('prns_host_uniffi.initialize()', native)
        host = generate.BINDINGS / 'typescript/prns_host_uniffi.ts'
        self.assertIn('personal-rns-expo/bindings/values', result[host])
        self.assertNotIn(host.with_name('prns_host_uniffi-ffi.ts'), result)

    def test_native_barrel_does_not_resolve_itself_through_metro(self):
        result = generate.assemble(self.sources(), 1)
        index = result[generate.BINDINGS / 'index.ts']
        native = result[generate.BINDINGS / 'index.native.ts']
        exports = [line for line in index.splitlines() if line.startswith('export ')]
        self.assertEqual(exports, [line for line in native.splitlines()
                                   if line.startswith('export ')])
        self.assertTrue(all('from "./typescript/' in line for line in exports))
        self.assertNotIn('from "./index"', native)

    def test_upstream_entrypoint_cannot_initialize_the_borrowed_host(self):
        sources = self.sources()
        upstream_index = generate.BINDINGS / 'typescript/index.ts'
        sources[upstream_index] = 'prns_host_uniffi.default.initialize();\n'
        result = generate.assemble(sources, 1)
        self.assertEqual(result, generate.assemble(self.sources(), 1))
        self.assertNotIn(upstream_index, result)
        self.assertIn(upstream_index, sources)

    def test_missing_canonical_host_fails(self):
        sources = self.sources()
        del sources[generate.BINDINGS / 'typescript/prns_host_uniffi.ts']
        with self.assertRaisesRegex(ValueError, 'canonical host'):
            generate.assemble(sources, 1)

    def test_stage_checks_tracked_outputs_and_writes_only_sdk(self):
        provider = {'libraryName': 'prns_chat_app'}
        with patch.object(generate.tooling, 'existing_outputs'), \
                patch.object(generate.tooling, 'ownership', return_value=([], [])), \
                patch.object(generate.sdk, 'image_provider', return_value=provider), \
                patch.object(generate.sdk, 'selected_destination'), \
                patch.object(generate, 'generated_files', return_value={}), \
                patch.object(generate.tooling, 'synchronize_outputs') as app, \
                patch.object(generate.sdk, 'generate') as sdk:
            generate.generate('cli', {}, stage_only=True)
        self.assertTrue(app.call_args.args[1])
        self.assertEqual(sdk.call_args.args, (provider, 'cli', {}, False, generate.SDK_PACKAGE))

    def test_sdk_and_domains_use_one_identical_provider(self):
        provider = generate.sdk.image_provider(generate.IMAGE)
        with patch.object(generate.tooling, 'generated_files', return_value=self.sources()) as shared:
            generate.generated_files('cli', {})
        recipe = shared.call_args.args[0]
        self.assertEqual(recipe.manifest, provider['manifest'])
        self.assertEqual(recipe.package, provider['package'])
        self.assertEqual(recipe.library_name, provider['libraryName'])
        self.assertEqual(recipe.crate_dir, provider['crateDirectory'])
        self.assertEqual(recipe.features, tuple(provider['features'] + provider['generationFeatures']))


if __name__ == '__main__':
    unittest.main()
