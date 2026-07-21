from __future__ import annotations

import importlib.util
from pathlib import Path
import tempfile
import threading
import unittest
from urllib.error import HTTPError
from urllib.request import urlopen


SCRIPT = Path(__file__).resolve().parents[1] / "serve-flasher-candidate.py"
SPEC = importlib.util.spec_from_file_location("serve_flasher_candidate", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"could not import {SCRIPT}")
SERVER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SERVER)


class CandidateServerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        (self.root / "index.html").write_text("candidate shell\n", encoding="utf-8")
        (self.root / "app.js").write_text("candidate asset\n", encoding="utf-8")
        self.server = SERVER.create_server(self.root, 0)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.base = f"http://127.0.0.1:{self.server.server_address[1]}"

    def tearDown(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)
        self.temporary.cleanup()

    def get(self, path: str):
        return urlopen(f"{self.base}{path}", timeout=2)

    def test_binds_only_loopback_and_serves_extensionless_spa_route(self) -> None:
        self.assertEqual(self.server.server_address[0], "127.0.0.1")
        with self.get("/flash") as response:
            self.assertEqual(response.read(), b"candidate shell\n")
            self.assertEqual(response.headers["Cache-Control"], "no-store")
            self.assertEqual(response.headers["Permissions-Policy"], "serial=(self)")

    def test_serves_existing_assets_without_spa_substitution(self) -> None:
        with self.get("/app.js") as response:
            self.assertEqual(response.read(), b"candidate asset\n")

    def test_missing_file_with_extension_is_a_real_404(self) -> None:
        with self.assertRaises(HTTPError) as raised:
            self.get("/missing.json")
        self.assertEqual(raised.exception.code, 404)
        raised.exception.close()

    def test_directory_listing_is_disabled(self) -> None:
        assets = self.root / "assets"
        assets.mkdir()
        (assets / "private-name.txt").write_text("public candidate data\n", encoding="utf-8")
        with self.assertRaises(HTTPError) as raised:
            self.get("/assets/")
        self.assertEqual(raised.exception.code, 404)
        raised.exception.close()

    def test_rejects_symlinked_candidate_content(self) -> None:
        outside = self.root.parent / f"{self.root.name}-outside"
        outside.write_text("outside\n", encoding="utf-8")
        linked = self.root / "linked.txt"
        try:
            linked.symlink_to(outside)
        except OSError as error:
            outside.unlink(missing_ok=True)
            self.skipTest(f"symlinks unavailable: {error}")
        try:
            with self.assertRaisesRegex(ValueError, "contains symlinks"):
                SERVER.create_server(self.root, 0)
        finally:
            outside.unlink(missing_ok=True)


if __name__ == "__main__":
    unittest.main()
