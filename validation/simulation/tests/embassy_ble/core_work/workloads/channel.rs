use super::*;

pub fn window_pressure(triple: &mut Triple<'_, '_>) {
    triple.reconnect(PRIMARY);
    let link = triple.app_links[0];
    let wire = triple.nodes[PRIMARY].wire.clone();
    let mut header = response_header(link);
    header.context = personal_rns::wire::WireContext::Channel;
    wire.arm(header, NonZeroUsize::MIN);
    let commands: Vec<_> = (0..3)
        .map(|marker| {
            triple.issue(
                PRIMARY,
                PrnsCommand::SendToChannel(SendToChannel {
                    link_id: link,
                    message_type: CHANNEL_KIND,
                    body: SendToChannelBody::from_slice(&[marker; 16]).expect("bounded channel"),
                }),
            )
        })
        .collect();
    let gate = wire.clone();
    triple.complete(async move {
        gate.held().await;
    });
    let settlements = triple.nodes[PRIMARY].events.snapshot();
    assert_eq!(settlements.iter().filter(|event| matches!(event, Event::Settled { id, settlement: CommandOutcome::Channel(Err(SendToChannelFailure::WindowFull)) } if commands.contains(id))).count(), 1, "bounded channel rejects pressure explicitly");
    healthy(triple);
    wire.release();
    let observed = triple.nodes[PRIMARY].events.clone();
    let expected = commands.clone();
    triple.drive_until(move || observed.snapshot().iter().filter(|event| matches!(event, Event::Settled { id, settlement: CommandOutcome::Channel(_) } if expected.contains(id))).count() == expected.len());
    for command in commands {
        assert_eq!(
            triple.nodes[PRIMARY]
                .events
                .snapshot()
                .iter()
                .filter(|event| matches!(event, Event::Settled { id, .. } if *id == command))
                .count(),
            1
        );
    }
    overlap(triple, 0x90);
}
