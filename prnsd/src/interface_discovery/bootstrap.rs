use std::collections::BTreeSet;

use personal_rns::config::{ConfiguredInterfaceLifecycle, DaemonPlan};
use personal_rns::from_plan::{PlanAttachments, PlanRuntimeContext};
use personal_rns::interfaces::InterfaceId;
use personal_rns::runtime::PrnsNodeHandle;
use tokio::sync::watch;

use crate::daemon::construct_configured_interfaces;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoConnectCapacity {
    pub online: usize,
    pub maximum: usize,
}

#[derive(Clone)]
pub struct MonitoredInterfaces {
    interfaces: watch::Sender<BTreeSet<InterfaceId>>,
}

impl MonitoredInterfaces {
    pub fn new(interfaces: impl IntoIterator<Item = InterfaceId>) -> Self {
        let (interfaces, _) = watch::channel(interfaces.into_iter().collect());
        Self { interfaces }
    }

    pub fn subscribe(&self) -> watch::Receiver<BTreeSet<InterfaceId>> {
        self.interfaces.subscribe()
    }

    pub(crate) fn add(&self, interfaces: impl IntoIterator<Item = InterfaceId>) {
        self.interfaces.send_modify(|monitored| {
            monitored.extend(interfaces);
        });
    }

    pub(crate) fn remove(&self, interfaces: impl IntoIterator<Item = InterfaceId>) {
        self.interfaces.send_modify(|monitored| {
            for interface in interfaces {
                monitored.remove(&interface);
            }
        });
    }
}

pub struct BootstrapInterfaces {
    plan: DaemonPlan,
    context: PlanRuntimeContext,
    active: PlanAttachments,
    monitored: MonitoredInterfaces,
}

impl BootstrapInterfaces {
    pub fn prepare(
        plan: &DaemonPlan,
        context: PlanRuntimeContext,
        active: PlanAttachments,
        monitored: MonitoredInterfaces,
    ) -> Result<Self, PlanAttachments> {
        if plan
            .discovery
            .enabled_policy()
            .and_then(|policy| policy.auto_connect().maximum())
            .is_none()
        {
            return Err(active);
        }
        let mut bootstrap_plan = plan.clone();
        bootstrap_plan
            .interfaces
            .retain(|interface| interface.lifecycle == ConfiguredInterfaceLifecycle::BootstrapOnly);
        if bootstrap_plan.interfaces.is_empty() {
            return Err(active);
        }
        Ok(Self {
            plan: bootstrap_plan,
            context,
            active: active.for_lifecycle(ConfiguredInterfaceLifecycle::BootstrapOnly),
            monitored,
        })
    }

    pub async fn run(
        mut self,
        handle: PrnsNodeHandle,
        mut capacities: watch::Receiver<Option<AutoConnectCapacity>>,
    ) {
        while capacities.changed().await.is_ok() {
            let Some(capacity) = *capacities.borrow_and_update() else {
                continue;
            };
            match bootstrap_action(!self.active.is_empty(), capacity) {
                BootstrapAction::Keep => {}
                BootstrapAction::Retire => self.retire(&handle).await,
                BootstrapAction::Restore => self.restore(&handle).await,
            }
        }
        let active = std::mem::take(&mut self.active);
        let interfaces = active.interfaces().collect::<Vec<_>>();
        self.monitored.remove(interfaces);
        active.detach(&handle).await;
    }

    pub fn into_active(self) -> PlanAttachments {
        self.active
    }

    async fn retire(&mut self, handle: &PrnsNodeHandle) {
        let active = std::mem::take(&mut self.active);
        let interfaces = active.interfaces().collect::<Vec<_>>();
        self.monitored.remove(interfaces.iter().copied());
        active.detach(handle).await;
        tracing::info!(
            event = "bootstrap_interfaces_retired",
            interfaces = interfaces.len(),
        );
    }

    async fn restore(&mut self, handle: &PrnsNodeHandle) {
        let constructed = construct_configured_interfaces(handle, &self.plan, &self.context).await;
        let interfaces = constructed
            .attached()
            .iter()
            .map(|interface| interface.id)
            .collect::<Vec<InterfaceId>>();
        let startup = constructed.startup;
        self.monitored.add(interfaces.iter().copied());
        self.active = constructed
            .into_attachments()
            .for_lifecycle(ConfiguredInterfaceLifecycle::BootstrapOnly);
        tracing::info!(
            event = "bootstrap_interfaces_restored",
            online = startup.online,
            listening = startup.listening,
            retrying = startup.retrying,
            failed = startup.failed,
            interfaces = interfaces.len(),
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BootstrapAction {
    Keep,
    Retire,
    Restore,
}

fn bootstrap_action(active: bool, capacity: AutoConnectCapacity) -> BootstrapAction {
    if active && capacity.online >= capacity.maximum {
        BootstrapAction::Retire
    } else if !active && capacity.online == 0 {
        BootstrapAction::Restore
    } else {
        BootstrapAction::Keep
    }
}

#[cfg(test)]
mod tests {
    use personal_rns::from_plan::{PlanAttachments, PlanRuntimeContext};
    use personal_rns::runtime::{
        ManuallyAttached, NoPersistence, PreConfiguredDestination, PrnsNode, PrnsNodeRecipe,
    };
    use personal_rns::storage::GrowableHeap;

    use super::{
        bootstrap_action, AutoConnectCapacity, BootstrapAction, BootstrapInterfaces,
        MonitoredInterfaces,
    };

    async fn wait_for_interface_count(
        handle: &personal_rns::runtime::PrnsNodeHandle,
        expected: usize,
    ) {
        let reached = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while handle.interfaces().len() != expected {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(
            reached.is_ok(),
            "interface registry expected {expected} entries but retained {}",
            handle.interfaces().len(),
        );
    }

    #[test]
    fn bootstrap_interfaces_retire_only_at_auto_connect_capacity() {
        assert_eq!(
            bootstrap_action(
                true,
                AutoConnectCapacity {
                    online: 2,
                    maximum: 3,
                }
            ),
            BootstrapAction::Keep
        );
        assert_eq!(
            bootstrap_action(
                true,
                AutoConnectCapacity {
                    online: 3,
                    maximum: 3,
                }
            ),
            BootstrapAction::Retire
        );
    }

    #[test]
    fn retired_bootstrap_interfaces_return_only_when_auto_connect_is_empty() {
        assert_eq!(
            bootstrap_action(
                false,
                AutoConnectCapacity {
                    online: 1,
                    maximum: 3,
                }
            ),
            BootstrapAction::Keep
        );
        assert_eq!(
            bootstrap_action(
                false,
                AutoConnectCapacity {
                    online: 0,
                    maximum: 3,
                }
            ),
            BootstrapAction::Restore
        );
    }

    #[tokio::test]
    async fn bootstrap_restore_and_retire_update_runtime_and_failure_monitor_together() {
        let plan = personal_rns::config::parse_and_plan(
            "[reticulum]\ndiscover_interfaces = Yes\nautoconnect_discovered_interfaces = 1\n\
             [interfaces]\n[[Bootstrap]]\ntype = TCPClientInterface\nenabled = Yes\nbootstrap_only = Yes\n\
             target_host = 127.0.0.1\ntarget_port = 9\n",
        )
        .expect("valid bootstrap configuration")
        .value;
        let node = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            remote_control: crate::test_support::remote_control_service(),
            pre_configured_destinations: std::iter::empty::<PreConfiguredDestination<'static>>(),
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: personal_rns::request_endpoints![],
            interfaces: ManuallyAttached,
            persistence: NoPersistence,
            on_event: |_event, _state: &()| {},
        });
        let handle = node.handle();
        let monitored = MonitoredInterfaces::new([]);
        let monitor = monitored.subscribe();
        let mut bootstrap = BootstrapInterfaces::prepare(
            &plan,
            PlanRuntimeContext::default(),
            PlanAttachments::default(),
            monitored,
        )
        .unwrap_or_else(|_| panic!("auto-connect and a bootstrap interface prepare a lifecycle"));
        let exercise = async {
            bootstrap.restore(&handle).await;
            wait_for_interface_count(&handle, 1).await;
            assert_eq!(monitor.borrow().len(), 1);

            bootstrap.retire(&handle).await;
            wait_for_interface_count(&handle, 0).await;
            assert!(monitor.borrow().is_empty());
        };
        tokio::select! {
            result = node.run() => panic!("test node stopped unexpectedly: {result:?}"),
            () = exercise => {}
        }
    }
}
