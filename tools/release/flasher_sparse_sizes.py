"""Release-boundary sparse transfer accounting and merged-image reduction gates."""

from __future__ import annotations

from flasher_manifest import target_artifacts


MERGED_BASELINES = {
    "heltec-v4": 7_643_152,
    "heltec-v4-r8": 7_643_152,
    "t-beam-supreme": 7_639_296,
    "xiao-esp32-c6": 1_309_056,
}
SPARSE_BASELINES = {
    board: MERGED_BASELINES[board]
    for board in ("heltec-v4", "heltec-v4-r8", "t-beam-supreme")
}
SHIPPING_BOARDS = {
    "heltec-v4",
    "heltec-v4-r8",
    "t-beam-supreme",
    "xiao-esp32-c6",
    "t-echo",
    "t114",
    "t096",
    "t1000-e",
}
REQUIRED_REDUCTION_PERCENT = 60


def build_report(manifest: dict) -> dict:
    release = manifest.get("release") if isinstance(manifest, dict) else None
    targets = manifest.get("targets") if isinstance(manifest, dict) else None
    if not isinstance(release, dict) or not isinstance(release.get("version"), str):
        raise ValueError("sparse-size report requires a manifest release version")
    if not isinstance(targets, list):
        raise ValueError("sparse-size report requires manifest targets")
    reports = []
    boards = set()
    for target in targets:
        if not isinstance(target, dict):
            raise ValueError("sparse-size report encountered a malformed target")
        board = target.get("board_slug")
        transport = target.get("transport")
        if not isinstance(board, str) or board in boards:
            raise ValueError("sparse-size report encountered an invalid board or parts list")
        parts = target_artifacts(target)
        boards.add(board)
        sizes = []
        for part in parts:
            size = part.get("size") if isinstance(part, dict) else None
            if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
                raise ValueError(f"sparse-size report encountered an invalid part for {board}")
            sizes.append(size)
        total = sum(sizes)
        source = target.get("source")
        source_bytes = 0
        if source is not None:
            source_bytes = source.get("size") if isinstance(source, dict) else None
            if (
                not isinstance(source_bytes, int)
                or isinstance(source_bytes, bool)
                or source_bytes <= 0
                or source_bytes > total
            ):
                raise ValueError(
                    f"sparse-size report encountered invalid source bytes for {board}"
                )
        code_total = total - source_bytes
        record = {
            "board_slug": board,
            "transport": transport,
            "part_count": len(parts),
            "total_bytes": total,
            "embedded_source_bytes": source_bytes,
            "code_payload_bytes": code_total,
        }
        baseline = MERGED_BASELINES.get(board)
        if baseline is not None:
            record["merged_baseline_bytes"] = baseline
            record["reduction_basis_points"] = (
                (baseline - code_total) * 10_000 // baseline
            )
            if board in SPARSE_BASELINES:
                maximum = baseline * (100 - REQUIRED_REDUCTION_PERCENT) // 100
                if code_total > maximum:
                    raise ValueError(
                        f"{board} sparse code payload {code_total} exceeds {maximum}; "
                        f"the {REQUIRED_REDUCTION_PERCENT}% reduction gate failed"
                    )
                record.update(
                    {
                        "gate": "passed",
                        "maximum_sparse_bytes": maximum,
                    }
                )
            else:
                record["gate"] = "aggregate-only"
        elif transport == "esp-serial":
            record["gate"] = "reported-no-merged-baseline"
        elif transport == "uf2-mass-storage":
            record["gate"] = "verified-uf2-not-gap-padded"
        elif transport == "nrf-serial-dfu":
            record["gate"] = "verified-nrf-serial-dfu-not-gap-padded"
        else:
            raise ValueError(f"sparse-size report encountered unknown transport {transport!r}")
        reports.append(record)
    if boards != SHIPPING_BOARDS:
        raise ValueError(f"sparse-size report target set differs: {sorted(boards)}")
    esp_reports = [record for record in reports if record["board_slug"] in MERGED_BASELINES]
    merged_total = sum(record["merged_baseline_bytes"] for record in esp_reports)
    sparse_total = sum(record["code_payload_bytes"] for record in esp_reports)
    maximum_sparse_total = (
        merged_total * (100 - REQUIRED_REDUCTION_PERCENT) // 100
    )
    if sparse_total > maximum_sparse_total:
        raise ValueError(
            f"aggregate ESP sparse total {sparse_total} exceeds {maximum_sparse_total}; "
            f"the {REQUIRED_REDUCTION_PERCENT}% reduction gate failed"
        )
    return {
        "schema": 1,
        "release_version": release["version"],
        "required_reduction_percent": REQUIRED_REDUCTION_PERCENT,
        "aggregate_esp": {
            "gate": "passed",
            "merged_baseline_bytes": merged_total,
            "sparse_total_bytes": sparse_total,
            "maximum_sparse_bytes": maximum_sparse_total,
            "reduction_basis_points": (merged_total - sparse_total)
            * 10_000
            // merged_total,
        },
        "targets": sorted(reports, key=lambda record: record["board_slug"]),
    }


def render_summary(report: dict) -> list[str]:
    aggregate = report["aggregate_esp"]
    lines = [
        "aggregate ESP: "
        f"{aggregate['sparse_total_bytes']} bytes versus "
        f"{aggregate['merged_baseline_bytes']} bytes merged "
        f"({aggregate['reduction_basis_points'] / 100:.2f}% reduction); "
        f"{aggregate['gate']}"
    ]
    for target in report["targets"]:
        line = (
            f"{target['board_slug']}: {target['total_bytes']} bytes "
            f"across {target['part_count']} part(s); {target['gate']}"
        )
        if target["embedded_source_bytes"]:
            line += (
                f"; {target['code_payload_bytes']} code bytes plus "
                f"{target['embedded_source_bytes']} embedded source bytes"
            )
        if "merged_baseline_bytes" in target:
            line += (
                f" versus {target['merged_baseline_bytes']} bytes merged "
                f"({target['reduction_basis_points'] / 100:.2f}% code reduction)"
            )
        lines.append(line)
    return lines
