"""Complete, source-bound test records for distributed embedded Miri execution."""
from __future__ import annotations

import hashlib
import json
import re
from dataclasses import dataclass
from pathlib import Path

from validation.hardening.embedded_miri_execution import TestExecution


class ShardError(RuntimeError):
    pass


@dataclass(frozen=True)
class Shard:
    index: int
    total: int

    def __post_init__(self) -> None:
        if (
            type(self.index) is not int or type(self.total) is not int
            or not 2 <= self.total <= 16 or not 0 <= self.index < self.total
        ):
            raise ShardError("Miri shard must select one index among 2 through 16 shards")

    @classmethod
    def parse(cls, value: str) -> Shard:
        if not re.fullmatch(r"[0-9]+/[0-9]+", value):
            raise ShardError("Miri shard must be INDEX/TOTAL")
        index, total = map(int, value.split("/"))
        return cls(index, total)

    def select(self, inventory: tuple[str, ...]) -> tuple[str, ...]:
        selected = inventory[self.index::self.total]
        if not selected:
            raise ShardError("Miri shard has no selected tests")
        return selected

    @classmethod
    def from_document(cls, value: object) -> Shard:
        if not isinstance(value, dict) or set(value) != {"index", "total"}:
            raise ShardError("invalid Miri shard identity")
        return cls(value["index"], value["total"])

    def document(self) -> dict[str, int]:
        return {"index": self.index, "total": self.total}


def validate_execution(result: TestExecution) -> None:
    if result.returncode != 0:
        raise ShardError(f"Miri test failed: {result.name}")
    output = result.output.replace(b"\r\n", b"\n")
    summaries = re.findall(rb"^test result: (.+)$", output, re.MULTILINE)
    if len(summaries) != 1 or not summaries[0].startswith(
        b"ok. 1 passed; 0 failed; 0 ignored;"
    ):
        raise ShardError(f"Miri did not complete exactly one unignored test: {result.name}")
    reported = re.findall(rb"^test (.+) \.\.\. ok$", output, re.MULTILINE)
    if [name.removesuffix(b" - should panic") for name in reported] != [result.name.encode()]:
        raise ShardError(f"Miri reported a different test: {result.name}")


def validated_inventory(value: object) -> tuple[str, ...]:
    if (
        not isinstance(value, list) or not value
        or any(not isinstance(name, str) or not name or "\n" in name for name in value)
        or len(value) != len(set(value))
    ):
        raise ShardError("Miri inventory must contain nonempty, unique test names")
    return tuple(value)


def write_fragment(
    path: Path, context: dict, shard: Shard, inventory: tuple[str, ...],
    results: tuple[TestExecution, ...],
) -> None:
    validated_inventory(list(inventory))
    selected = shard.select(inventory)
    observed = {result.name for result in results}
    if observed != set(selected) or len(observed) != len(results):
        raise ShardError("Miri shard results do not cover its exact assigned inventory")
    for result in results:
        validate_execution(result)
    path.write_text(
        json.dumps(
            {
                "schema": 1,
                "context": context,
                "shard": shard.document(),
                "inventory": inventory,
                "tests": [
                    {
                        "name": result.name,
                        "bytes": len(result.output),
                        "sha256": hashlib.sha256(result.output).hexdigest(),
                    }
                    for result in results
                ],
            },
            indent=2, sort_keys=True,
        ) + "\n",
        encoding="utf-8",
    )


@dataclass(frozen=True)
class CollectedModel:
    context: dict
    inventory: tuple[str, ...]
    outputs: tuple[bytes, ...]
    fragments: tuple[dict, ...]


def collect_model(
    directories: tuple[Path, ...], component: str, model: str,
    expected_context: dict,
) -> CollectedModel:
    shared_context = None
    shared_inventory = None
    outputs: dict[str, bytes] = {}
    fragments = []
    for index, directory in enumerate(directories):
        shard = Shard(index, len(directories))
        fragment = directory / f"{component}-{model}.shard.json"
        try:
            document = json.loads(fragment.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise ShardError(f"cannot read Miri shard {fragment}: {error}") from error
        if (
            not isinstance(document, dict)
            or set(document) != {"schema", "context", "shard", "inventory", "tests"}
            or type(document["schema"]) is not int or document["schema"] != 1
        ):
            raise ShardError(f"invalid Miri shard identity or schema: {fragment}")
        if Shard.from_document(document["shard"]) != shard:
            raise ShardError(f"Miri shard identity differs: {fragment}")
        context = document["context"]
        if not isinstance(context, dict) or any(
            context.get(key) != value for key, value in expected_context.items()
        ):
            raise ShardError(f"Miri shard source or execution contract differs: {fragment}")
        inventory = validated_inventory(document["inventory"])
        if shared_context is None:
            shared_context = context
            shared_inventory = inventory
        if context != shared_context or inventory != shared_inventory:
            raise ShardError(f"Miri shards disagree on their context or full inventory: {fragment}")
        selected = shard.select(inventory)
        tests = document["tests"]
        if not isinstance(tests, list) or len(tests) != len(selected):
            raise ShardError(f"Miri shard has incomplete test records: {fragment}")
        seen: set[str] = set()
        for test in tests:
            if not isinstance(test, dict) or set(test) != {"name", "bytes", "sha256"}:
                raise ShardError(f"invalid Miri test record: {fragment}")
            name = test["name"]
            if not isinstance(name, str) or name not in selected or name in seen or name in outputs:
                raise ShardError(f"unexpected or duplicate Miri test record: {fragment}")
            seen.add(name)
            log = directory / f"{component}-{model}-tests" / f"{selected.index(name):04}.log"
            if directory.resolve() not in log.resolve().parents:
                raise ShardError(f"Miri test log escapes its shard: {log}")
            try:
                output = log.read_bytes()
            except OSError as error:
                raise ShardError(f"cannot read Miri test log {log}: {error}") from error
            if (
                type(test["bytes"]) is not int or test["bytes"] != len(output)
                or test["sha256"] != hashlib.sha256(output).hexdigest()
            ):
                raise ShardError(f"Miri test log fingerprint differs: {log}")
            validate_execution(TestExecution(name, 0, output))
            outputs[name] = output
        if seen != set(selected):
            raise ShardError(f"Miri shard omitted assigned tests: {fragment}")
        fragments.append(document)
    if shared_context is None or shared_inventory is None or set(outputs) != set(shared_inventory):
        raise ShardError("Miri shards do not cover the complete discovered test inventory")
    return CollectedModel(
        shared_context, shared_inventory, tuple(outputs[name] for name in shared_inventory),
        tuple(fragments),
    )
