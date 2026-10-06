from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from validation.hardening.embedded_miri_execution import TestExecution
from validation.hardening.embedded_miri_shards import (
    Shard, ShardError, collect_model, validate_execution, write_fragment,
)


NAMES = ("fixture::one", "fixture::two", "fixture::three", "fixture::four")
CONTEXT = {"commit": "a" * 40, "model": "stacked", "toolchain": "pinned", "flags": []}


def output_for(name: str) -> bytes:
    return (
        f"test={name}\nrunning 1 test\ntest {name} ... ok\n\n"
        "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out\n"
    ).encode()


def fixture(root: Path) -> tuple[Path, ...]:
    directories = tuple(root / str(index) for index in range(2))
    for index, directory in enumerate(directories):
        shard = Shard(index, 2)
        logs = directory / "component-stacked-tests"
        logs.mkdir(parents=True)
        results = []
        for position, name in enumerate(shard.select(NAMES)):
            output = output_for(name)
            (logs / f"{position:04}.log").write_bytes(output)
            results.append(TestExecution(name, 0, output))
        write_fragment(
            directory / "component-stacked.shard.json", CONTEXT, shard, NAMES,
            tuple(reversed(results)),
        )
    return directories


class EmbeddedMiriShardTests(unittest.TestCase):
    def test_partitions_cover_the_whole_inventory_once(self) -> None:
        for total in range(2, 17):
            names = tuple(f"case::{index}" for index in range(75))
            selected = [name for index in range(total) for name in Shard(index, total).select(names)]
            self.assertCountEqual(selected, names)
        for value in ("-1/4", "4/4", "0/1", "0/17", "0/0", "0", "one/four"):
            with self.subTest(value=value), self.assertRaises(ShardError):
                Shard.parse(value)
        with self.assertRaises(ShardError):
            Shard(False, 4)
        with self.assertRaises(ShardError):
            Shard(3, 4).select(("one",))

    def test_collection_preserves_discovery_order_despite_completion_order(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directories = fixture(Path(temporary))
            collected = collect_model(directories, "component", "stacked", CONTEXT)
        self.assertEqual(collected.context, CONTEXT)
        self.assertEqual(collected.inventory, NAMES)
        self.assertEqual(collected.outputs, tuple(output_for(name) for name in NAMES))

    def test_collection_rejects_stale_mixed_incomplete_and_duplicate_records(self) -> None:
        mutations = {
            "wrong source": lambda value: value["context"].update(commit="b" * 40),
            "wrong model": lambda value: value["context"].update(model="tree"),
            "mixed toolchains": lambda value: value["context"].update(toolchain="other"),
            "changed flags": lambda value: value["context"].update(flags=["disabled-check"]),
            "wrong shard": lambda value: value["shard"].update(index=1),
            "boolean shard": lambda value: value["shard"].update(index=False),
            "different inventory": lambda value: value["inventory"].append("extra"),
            "duplicate inventory": lambda value: value["inventory"].append(NAMES[0]),
            "missing test": lambda value: value["tests"].pop(),
            "duplicate test": lambda value: value["tests"].__setitem__(0, value["tests"][1]),
            "unknown test": lambda value: value["tests"][0].update(name="unknown"),
            "wrong fingerprint": lambda value: value["tests"][0].update(sha256="0" * 64),
            "wrong length": lambda value: value["tests"][0].update(bytes=1),
            "boolean schema": lambda value: value.update(schema=True),
        }
        for label, mutate in mutations.items():
            with self.subTest(label=label), tempfile.TemporaryDirectory() as temporary:
                directories = fixture(Path(temporary))
                path = directories[0] / "component-stacked.shard.json"
                document = json.loads(path.read_text())
                mutate(document)
                path.write_text(json.dumps(document))
                with self.assertRaises(ShardError):
                    collect_model(directories, "component", "stacked", {"commit": CONTEXT["commit"]})

    def test_collection_rejects_missing_or_changed_logs_and_fragments(self) -> None:
        for kind in ("missing fragment", "missing log", "changed log", "escaping log"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                directories = fixture(root)
                log = directories[0] / "component-stacked-tests/0000.log"
                if kind == "missing fragment":
                    (directories[0] / "component-stacked.shard.json").unlink()
                elif kind == "changed log":
                    log.write_bytes(log.read_bytes() + b"changed")
                elif kind == "escaping log":
                    outside = root / "outside.log"
                    outside.write_bytes(log.read_bytes())
                    log.unlink()
                    log.symlink_to(outside)
                else:
                    log.unlink()
                with self.assertRaises(ShardError):
                    collect_model(directories, "component", "stacked", CONTEXT)

    def test_success_requires_the_exact_named_test_and_no_ignored_tests(self) -> None:
        good = output_for(NAMES[0])
        validate_execution(TestExecution(NAMES[0], 0, good.replace(b"\n", b"\r\n")))
        validate_execution(TestExecution(NAMES[0], 0, good.replace(b" ... ok", b" - should panic ... ok")))
        cases = (
            TestExecution(NAMES[0], 1, good),
            TestExecution(NAMES[0], 0, good.replace(b"0 ignored", b"1 ignored")),
            TestExecution(NAMES[0], 0, good.replace(b"1 passed", b"0 passed")),
            TestExecution(NAMES[0], 0, good + good),
            TestExecution(NAMES[1], 0, good),
        )
        for result in cases:
            with self.subTest(result=result), self.assertRaises(ShardError):
                validate_execution(result)

    def test_writer_refuses_to_attest_partial_or_duplicate_execution(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "fragment.json"
            result = TestExecution(NAMES[0], 0, output_for(NAMES[0]))
            for results in ((result,), (result, result)):
                with self.assertRaises(ShardError):
                    write_fragment(path, CONTEXT, Shard(0, 2), NAMES, results)
                self.assertFalse(path.exists())


if __name__ == "__main__":
    unittest.main()
