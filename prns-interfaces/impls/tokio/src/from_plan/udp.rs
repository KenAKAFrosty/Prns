use prns_config::UdpFlowPlan;

use crate::host_network::{resolve_udp_endpoint, udp_ephemeral_bind};
use crate::udp::UdpInterface;

use super::{AttachmentResult, InterfaceConstruction};

pub(super) async fn stand_up(
    construction: InterfaceConstruction<'_>,
    flow: &UdpFlowPlan,
) -> AttachmentResult {
    let opened = match flow {
        UdpFlowPlan::ReceiveOnly { listen } => match resolve_udp_endpoint(listen).await {
            Ok(listen) => {
                UdpInterface::bind_receive_with_policy(listen, construction.interface.policy).await
            }
            Err(error) => Err(error),
        },
        UdpFlowPlan::SendOnly { forward } => match resolve_udp_endpoint(forward).await {
            Ok(forward) => {
                UdpInterface::bind_send_with_policy(
                    udp_ephemeral_bind(),
                    forward,
                    construction.interface.policy,
                )
                .await
            }
            Err(error) => Err(error),
        },
        UdpFlowPlan::Bidirectional { listen, forward } => {
            match (
                resolve_udp_endpoint(listen).await,
                resolve_udp_endpoint(forward).await,
            ) {
                (Ok(listen), Ok(forward)) => {
                    UdpInterface::bind_with_policy(listen, forward, construction.interface.policy)
                        .await
                }
                (Err(error), _) | (_, Err(error)) => Err(error),
            }
        }
    };
    let udp = opened?;
    let attached = construction.attach(udp);
    Ok(attached.id())
}
