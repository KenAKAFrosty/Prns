pub(super) struct Args {
    pub(super) scenario: benchmarks::ScenarioId,
    pub(super) initiator: String,
    pub(super) responder: String,
    pub(super) duration_ms: Option<u64>,
    pub(super) sample_index: u32,
    pub(super) run_id: String,
    pub(super) smoke: bool,
    pub(super) write: bool,
}

pub(super) struct SuiteArgs {
    pub(super) samples: u32,
    pub(super) duration_ms: u64,
    pub(super) dry_run: bool,
    pub(super) smoke: bool,
    pub(super) only_cells: Option<std::collections::BTreeSet<usize>>,
    pub(super) output: Option<std::path::PathBuf>,
    pub(super) suite_id: Option<String>,
}

pub(super) enum RunnerCommand {
    Run(Args),
    Suite(SuiteArgs),
}

const USAGE: &str = "usage:\n  benchmark_runner run <scenario> [--initiator personal-rns] [--responder rns-1.4.0-compiled] [options]\n  benchmark_runner suite release [--samples 3] [--duration-ms 30000] [--output DIR] [--suite-id ID] [--only-cells 7,9,10] [--dry-run|--smoke]";

pub(super) fn parse_args() -> RunnerCommand {
    let mut arguments = std::env::args().skip(1);
    match arguments.next().as_deref() {
        Some("run") => parse_run(arguments.collect()),
        Some("suite") => parse_suite(arguments.collect()),
        _ => panic!("{USAGE}"),
    }
}

fn parse_run(values: Vec<String>) -> RunnerCommand {
    let mut values = values.into_iter();
    let scenario = values
        .next()
        .unwrap_or_else(|| panic!("{USAGE}"))
        .parse()
        .unwrap_or_else(|error| panic!("{error}\n{USAGE}"));
    let mut args = Args {
        scenario,
        initiator: "personal-rns".into(),
        responder: "personal-rns".into(),
        duration_ms: None,
        sample_index: 0,
        run_id: uuid::Uuid::new_v4().to_string(),
        smoke: false,
        write: true,
    };
    let mut values = values.peekable();
    while let Some(flag) = values.next() {
        let value = |values: &mut std::iter::Peekable<std::vec::IntoIter<String>>, name: &str| {
            values
                .next()
                .unwrap_or_else(|| panic!("{name} needs a value"))
        };
        match flag.as_str() {
            "--initiator" => args.initiator = value(&mut values, "--initiator"),
            "--responder" => args.responder = value(&mut values, "--responder"),
            "--duration-ms" => {
                args.duration_ms = Some(
                    value(&mut values, "--duration-ms")
                        .parse()
                        .expect("duration milliseconds"),
                )
            }
            "--sample-index" => {
                args.sample_index = value(&mut values, "--sample-index")
                    .parse()
                    .expect("sample index")
            }
            "--run-id" => args.run_id = value(&mut values, "--run-id"),
            "--smoke" => {
                args.smoke = true;
                args.write = false;
            }
            "--no-write" => args.write = false,
            other => panic!("unknown run option {other}\n{USAGE}"),
        }
    }
    RunnerCommand::Run(args)
}

fn parse_suite(values: Vec<String>) -> RunnerCommand {
    let mut values = values.into_iter();
    assert_eq!(values.next().as_deref(), Some("release"), "{USAGE}");
    let mut args = SuiteArgs {
        samples: 3,
        duration_ms: 30_000,
        dry_run: false,
        smoke: false,
        only_cells: None,
        output: None,
        suite_id: None,
    };
    while let Some(flag) = values.next() {
        match flag.as_str() {
            "--samples" => {
                args.samples = values
                    .next()
                    .and_then(|value| value.parse().ok())
                    .expect("sample count")
            }
            "--duration-ms" => {
                args.duration_ms = values
                    .next()
                    .and_then(|value| value.parse().ok())
                    .expect("duration milliseconds")
            }
            "--dry-run" => args.dry_run = true,
            "--smoke" => {
                args.smoke = true;
                args.samples = 1;
                args.duration_ms = 500;
            }
            "--only-cells" => {
                let cells = values
                    .next()
                    .expect("--only-cells needs a comma-separated list")
                    .split(',')
                    .map(|value| {
                        value
                            .trim()
                            .parse::<usize>()
                            .expect("cell numbers must be positive integers")
                    })
                    .collect::<std::collections::BTreeSet<_>>();
                assert!(
                    !cells.is_empty() && cells.iter().all(|cell| *cell > 0),
                    "cell numbers are one-based"
                );
                args.only_cells = Some(cells);
            }
            "--output" => {
                args.output = Some(std::path::PathBuf::from(
                    values.next().expect("--output needs a directory"),
                ));
            }
            "--suite-id" => {
                args.suite_id = Some(values.next().expect("--suite-id needs an identifier"));
            }
            other => panic!("unknown suite option {other}\n{USAGE}"),
        }
    }
    assert!(args.samples > 0, "samples must be positive");
    RunnerCommand::Suite(args)
}
