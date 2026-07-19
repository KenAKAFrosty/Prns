pub(super) struct Args {
    pub(super) scenario: String,
    pub(super) initiator: String,
    pub(super) responder: String,
    pub(super) relay: String,
    pub(super) duration_ms: Option<u64>,
}

pub(super) fn parse_args() -> Args {
    let mut args = Args {
        scenario: "single-firehose".into(),
        initiator: "self".into(),
        responder: "self".into(),
        relay: "self".into(),
        duration_ms: None,
    };
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--initiator" => args.initiator = argv.next().expect("impl name"),
            "--responder" => args.responder = argv.next().expect("impl name"),
            "--relay" => args.relay = argv.next().expect("impl name"),
            "--duration-ms" => {
                args.duration_ms = Some(argv.next().and_then(|v| v.parse().ok()).expect("ms"));
            }
            other if !other.starts_with("--") => args.scenario = other.into(),
            other => panic!("unknown flag {other}"),
        }
    }
    args
}
