from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Callable, Mapping

from validation.interop.harness import (
    InteropCase,
    PeerSpec,
    PortLease,
    cargo_example,
    environment,
    reference_python,
)


ROOT = Path(__file__).resolve().parents[3]
MANIFEST = ROOT / "validation/integration/Cargo.toml"
STOCK_TRANSPORT = ROOT / "validation/interop/peers/rns_multihop_transport.py"
STOCK_ENDPOINT = ROOT / "validation/interop/peers/rns_multihop_endpoint.py"


class StartPolicy(Enum):
    IMMEDIATE = "immediate"
    SYNCHRONIZED = "synchronized"


Preparation = Callable[[Path, Path], Mapping[str, Mapping[str, object]]]


@dataclass(frozen=True)
class MixedTransportScenario:
    endpoint_mode: str
    left_success: str
    right_success: str
    timeout_seconds: float
    start_policy: StartPolicy
    prepare: Preparation


def no_preparation(_python: Path, _work: Path) -> Mapping[str, Mapping[str, object]]:
    return {}


def run_mixed_transport(scenario: MixedTransportScenario) -> None:
    python = reference_python()
    daemon = cargo_example(MANIFEST, "mixed_multihop_daemon")
    with (
        PortLease() as left_port,
        PortLease() as prns_port,
        PortLease() as right_port,
        InteropCase() as case,
    ):
        prepared = scenario.prepare(python, case.work)
        start_path = case.work / "start"

        def endpoint_environment(role: str, port: int) -> dict[str, str]:
            values: dict[str, object] = {
                "RNS_MULTIHOP_ROLE": role,
                "RNS_MULTIHOP_ENDPOINT_PORT": port,
                "RNS_MULTIHOP_CONFIG_DIR": case.work / f"{role}-config",
            }
            values.update(prepared.get(role, {}))
            if scenario.start_policy is StartPolicy.SYNCHRONIZED:
                values["RNS_MULTIHOP_START"] = start_path
            return environment(values)

        right = case.start_reference_rns(
            PeerSpec(
                "right stock RNS endpoint",
                (str(python), str(STOCK_ENDPOINT), scenario.endpoint_mode),
                endpoint_environment("right", right_port.port),
            ),
            right_port,
        )
        case.wait_for(
            right,
            f"MULTIHOP_ENDPOINT_UP role=right scenario={scenario.endpoint_mode}",
            10,
        )
        prns = case.start(
            PeerSpec(
                "Prns multihop transport",
                (str(daemon),),
                environment(
                    {
                        "PRNS_MULTIHOP_LISTEN_PORT": prns_port.port,
                        "PRNS_MULTIHOP_PEER": f"127.0.0.1:{right_port.port}",
                    }
                ),
            ),
            prns_port,
        )
        case.wait_for(prns, "MIXED_MULTIHOP_READY", 10)
        transport = case.start_reference_rns(
            PeerSpec(
                "stock RNS multihop transport",
                (str(python), str(STOCK_TRANSPORT)),
                environment(
                    {
                        "RNS_MULTIHOP_LISTEN_PORT": left_port.port,
                        "RNS_MULTIHOP_PEER_PORT": prns_port.port,
                        "RNS_MULTIHOP_CONFIG_DIR": case.work / "transport-config",
                    }
                ),
            ),
            left_port,
        )
        case.wait_for(transport, "MULTIHOP_TRANSPORT_UP", 10)
        left = case.start_reference_rns(
            PeerSpec(
                "left stock RNS endpoint",
                (str(python), str(STOCK_ENDPOINT), scenario.endpoint_mode),
                endpoint_environment("left", left_port.port),
            )
        )
        case.wait_for(
            left,
            f"MULTIHOP_ENDPOINT_UP role=left scenario={scenario.endpoint_mode}",
            10,
        )
        if scenario.start_policy is StartPolicy.SYNCHRONIZED:
            start_path.write_text("start", encoding="utf-8")
        case.wait_for_all(
            [(left, scenario.left_success), (right, scenario.right_success)],
            scenario.timeout_seconds,
            required_peers=(prns, transport),
        )
