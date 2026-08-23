from pathlib import Path

from validation.interop.harness import (
    InteropCase,
    PeerSpec,
    PortLease,
    candidate_peer,
    case_main,
    environment,
    reference_python,
)


ROOT = Path(__file__).resolve().parents[3]
STOCK_PEER = ROOT / "validation/interop/peers/rns_buffer_stream_server.py"
SUCCESS = "PASS: stock RNS and Prns exchanged exact Buffer streams with independent boundaries and clean EOF"


def run() -> None:
    python = reference_python()
    candidate = candidate_peer()
    with PortLease() as port, InteropCase() as case:
        stock = case.start_reference_rns(
            PeerSpec(
                "stock RNS Buffer stream server",
                (str(python), str(STOCK_PEER)),
                environment({"PRNS_BUFFER_STREAM_PORT": port.port}),
            ),
            port,
        )
        case.wait_for(stock, "BUFFER_STREAM_SERVER_UP", 10)
        prns = case.start(
            PeerSpec(
                "Prns Buffer stream client",
                (str(candidate), "buffer-stream"),
                environment({"PRNS_TCP_TARGET": f"127.0.0.1:{port.port}"}),
            )
        )
        case.wait_for_all(
            [
                (stock, "STOCK_BUFFER_STREAM_OK received=4096 sent=4096 eof=1"),
                (prns, "PRNS_BUFFER_STREAM_OK received=4096 sent=4096 eof=1"),
            ],
            45,
        )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
