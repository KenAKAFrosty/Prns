use prns_config::{PipeCommandPlan, PipeRespawnDelay as PlannedPipeRespawnDelay};

use crate::pipe::{PipeInterface, PipeRespawnDelay};

use super::{AttachmentResult, InterfaceConstruction};

pub(super) fn stand_up(
    construction: InterfaceConstruction<'_>,
    command: &PipeCommandPlan,
    respawn_delay: PlannedPipeRespawnDelay,
) -> AttachmentResult {
    let respawn_delay = PipeRespawnDelay::new(respawn_delay.get());
    let argv = command.argv().to_vec();
    let pipe = PipeInterface::with_policy(
        move || {
            let argv = argv.clone();
            async move { crate::pipe::spawn(&argv).await }
        },
        respawn_delay,
        construction.interface.policy,
        command.source().as_bytes(),
    );
    let attached = construction.attach(pipe);
    Ok(attached.id())
}
