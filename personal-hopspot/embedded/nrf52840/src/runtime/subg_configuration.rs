use personal_hopspot_core as hopspot;
use personal_rns::interfaces::subghz::SubGConfigurationState;
use personal_rns::lora::{LoRaApplyOutcome, LoRaController};
use personal_rns::runtime::RemoteControlHostCommandError;

pub(super) type ConfigurationStore =
    hopspot::SubGConfigurationStore<super::learned_state::BoardFlash>;

pub(super) async fn apply_subg_configuration(
    controller: &mut LoRaController<'static>,
    store: &mut ConfigurationStore,
    active: &mut SubGConfigurationState,
    requested: SubGConfigurationState,
) -> Result<(), RemoteControlHostCommandError> {
    hopspot::apply_remote_subg_configuration(
        async |configuration| match controller.apply_configuration(configuration).await {
            LoRaApplyOutcome::Applied => Ok(()),
            LoRaApplyOutcome::Rejected | LoRaApplyOutcome::IdentityExhausted => {
                Err(RemoteControlHostCommandError::ApplyFailed)
            }
        },
        store,
        active,
        requested,
    )
    .await
}
