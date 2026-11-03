//! A generic record consumer over real Reticulum Links on a controlled local
//! carrier. Fixture enrollment and temporary plaintext stores are demonstration
//! setup, not production provisioning or protected phone storage.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "../tests/support/mod.rs"]
mod support;
#[path = "support/telemetry_projection.rs"]
mod telemetry_projection;

use prns_groups::{
    fixture, run, ExecutionContext, Intent, OriginalScope, Outcome, PublicationIntent, Request,
    RequestId, RunnerControl, RunnerOptions, Targets,
};
use prns_groups_reticulum::{Network, SyncBudget};
use prns_groups_service::Timestamp;
use std::{sync::Arc, time::Duration};
use telemetry_projection::{Projection, Reading, Refresh, NAMESPACE, SCHEMA};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut nodes = support::wire::Nodes::open().await;
    let identities: Vec<_> = nodes.owners[..2]
        .iter()
        .map(|owner| owner.client().identity_hash().unwrap())
        .collect();
    let enrollment = fixture::generate("Telemetry example", &identities, Timestamp(support::NOW))?;
    let directory = tempfile::tempdir()?;
    let workers: Vec<_> = (0..2)
        .map(|i| {
            support::Worker::open(
                directory.path().join(format!("sensor-{i}.redb")),
                enrollment[i].clone(),
                identities[i],
            )
        })
        .collect();
    let group = workers[0].client.network_config().await?.group;
    let mut controls = Vec::new();
    let mut runners = Vec::new();
    for (i, worker) in workers.iter().enumerate() {
        let config = worker.client.network_config().await?;
        let network = Arc::new(Network::new(
            nodes.owners[i].client(),
            nodes.owners[i].client().destination_hashes().unwrap()[0],
            config.peers,
            support::wire::limits(),
        )?);
        let (control, stop) = tokio::sync::watch::channel(RunnerControl::Running);
        controls.push(control);
        runners.push(tokio::spawn(run(
            worker.client.clone(),
            network,
            nodes.jobs[i].take().unwrap(),
            RunnerOptions {
                poll_interval: Duration::from_secs(1),
                retry_initial: Duration::from_millis(100),
                retry_max: Duration::from_secs(5),
                budget: SyncBudget {
                    max_requests: 4,
                    max_envelopes: 1,
                    max_bytes: 16384,
                },
            },
            Arc::new(support::TestClock),
            stop,
        )));
    }
    let reading = Reading {
        sensor: 3,
        centi_celsius: 2150,
    };
    let config = workers[0].client.network_config().await?;
    let Outcome::Published(id) = workers[0]
        .client
        .execute(
            Request {
                id: RequestId::from(prns_groups::RequestToken::new(support::NOW, [42; 20])?),
                original_scope: Some(OriginalScope::Control {
                    control: config.control,
                    incarnation: config.incarnation,
                }),
                intent: Intent::Publish(PublicationIntent {
                    group,
                    namespace: NAMESPACE,
                    schema: SCHEMA,
                    targets: Targets::Explicit(
                        config.peers.iter().map(|peer| peer.endpoint).collect(),
                    ),
                    body: reading.encode().to_vec(),
                }),
            },
            ExecutionContext {
                clock: support::clock(),
            },
        )
        .await?
    else {
        return Err("unexpected group outcome".into());
    };
    let mut projection = Projection::new(256)?;
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if projection
                .refresh(&workers[1].client, group, support::clock())
                .await?
                == Refresh::Incomplete
            {
                eprintln!(
                    "Telemetry projection refresh incomplete; retrying within the demo deadline."
                );
            }
            if projection.readings.get(&id) == Some(&reading)
                && workers[0]
                    .client
                    .delivery_progress(group, config.incarnation, id, support::clock())
                    .await?
                    .is_some_and(|progress| progress.summary.all_received())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok::<_, prns_groups::Error>(())
    })
    .await??;
    assert_eq!(projection.readings.get(&id), Some(&reading));
    println!(
        "Sensor 3: 21.50 °C; receiver committed the record and sender has its device receipt."
    );
    for control in controls {
        control.send(RunnerControl::Stop)?;
    }
    for runner in runners {
        runner.await??;
    }
    let expired = Timestamp(support::NOW + 15 * 86400);
    let expired = prns_groups_core::ClockReading::Trusted(prns_groups_core::ClockInterval::new(
        expired, expired,
    )?);
    workers[1].client.maintain(expired).await?;
    assert_eq!(
        projection
            .refresh(&workers[1].client, group, expired)
            .await?,
        Refresh::Current
    );
    assert!(!projection.readings.contains_key(&id));
    println!(
        "Retention invalidation rebuilt the consumer projection and removed the expired reading."
    );
    for worker in workers {
        worker.stop();
    }
    nodes.stop().await;
    Ok(())
}
