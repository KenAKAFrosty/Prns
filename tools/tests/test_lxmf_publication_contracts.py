from __future__ import annotations
import ast
from pathlib import Path
import re
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]


def selected(path):
    return {node.targets[0].id: ast.literal_eval(node.value)
            for node in ast.parse((ROOT / path).read_text()).body
            if isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name)
            and node.targets[0].id in {"GRAPHS", "GRAPH_FEATURES"}}


class LxmfPublicationContracts(unittest.TestCase):
    def test_selected_graphs_preserve_feature_isolation(self):
        for path, prefix in (("tools/repo/generate-third-party-notices.py", "LXMF "),
                             ("validation/security/unsafe-audit.py", "lxmf-")):
            data = selected(path)
            rows = [row for row in data["GRAPHS"] if row[0].startswith(prefix)]
            self.assertEqual(len(rows), 7)
            self.assertEqual(len({row[0] for row in data["GRAPHS"]}), len(data["GRAPHS"]))
            for name, manifest, _ in rows:
                self.assertTrue((ROOT / manifest).is_file())
                if "mailbox" in name.lower():
                    self.assertEqual(data["GRAPH_FEATURES"][name], ("tokio-host", "redb-mailbox"))
                elif "wire" in name.lower():
                    self.assertNotIn(name, data["GRAPH_FEATURES"])
                else:
                    self.assertEqual(manifest, "prns-lxmf/service/tests/fixtures/consumer/Cargo.toml")
                    self.assertNotIn("tokio-host", data["GRAPH_FEATURES"][name])
        rows = [line for line in (ROOT / "validation/security/deps-audit.sh").read_text().splitlines()
                if re.match(r'    "lxmf-', line)]
        self.assertEqual(len(rows), 7)

    def test_storage_feature_is_qualified_when_lxmf_shares_the_workspace(self):
        for path, graph in (("tools/repo/generate-third-party-notices.py", "Storage redb"),
                            ("validation/security/unsafe-audit.py", "storage-redb")):
            self.assertEqual(selected(path)["GRAPH_FEATURES"][graph], ("prns-storage/redb",))
        self.assertIn(
            '"storage-redb|prns-storage/Cargo.toml|x86_64-unknown-linux-gnu|prns-storage/redb"',
            (ROOT / "validation/security/deps-audit.sh").read_text(),
        )

    def test_owner_checks_are_required_and_prepared(self):
        manifest = tomllib.loads((ROOT / "validation/manifest.toml").read_text())
        suites = {suite["id"]: suite for suite in manifest["suite"]}
        self.assertEqual(suites["lxmf-direct"]["command"], ["./tools/prns", "build", "lxmf", "verify"])
        portable = suites["standalone-libraries-portable"]["command"]
        self.assertIn("--no-default-features", portable)
        self.assertIn("thumbv7em-none-eabihf", portable)
        self.assertIn("prns-lxmf-wire", portable)
        self.assertIn("prns-lxmf", portable)
        if "prns-contact-card" in portable:
            self.assertTrue((ROOT / "prns-contact-card/Cargo.toml").is_file())
        ci = (ROOT / ".github/workflows/ci.yml").read_text()
        for text in ("--suite lxmf-direct", "--suite standalone-libraries-portable", "LXMF_DIRECT_RESULT", "rustup toolchain install 1.90.0 --profile minimal"):
            self.assertIn(text, ci)
        release = (ROOT / ".github/workflows/release-readiness.yml").read_text()
        self.assertIn("matrix.id == 'lxmf-direct' || matrix.id == 'standalone-libraries-portable'", release)
        self.assertIn("Prepare isolated LXMF MSRV consumers", release)
        parity = (ROOT / "validation/hygiene/pre-push-ci-parity.py").read_text()
        self.assertIn('LXMF direct exchange (macOS)', parity)
        self.assertIn('"standalone-libraries"', parity)


if __name__ == "__main__":
    unittest.main()
