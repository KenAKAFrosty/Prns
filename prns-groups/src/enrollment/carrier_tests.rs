//! Real MLS receive cuts use the same engine as the network carrier.
use super::*;
use crate::admission::{Access, Authorization, CarrierEffectId, CarrierRecovery, Settlement};
use crate::client::{Command, Job, Reply as NativeReply};
use crate::session::carrier::fault::{self, Cut};
use crate::{
    Attachment, Error, ExecutionContext, GroupManager, Intent, OriginalScope, PublicationIntent,
    Request, RequestId, Targets,
};
use prns_groups_core::{ClockInterval, ClockReading, NamespaceId, SchemaId, Timestamp};
use prns_groups_reticulum::{SyncBudget, SyncOptions};
use prns_groups_service::{Incoming, InventoryCursor, InventoryKind};
use std::sync::Arc;
use tokio::sync::{oneshot, Notify};

#[derive(Default)]
struct Gate {
    pending: Option<CarrierEffectId>,
    paused: bool,
    admitted: usize,
    settled: Vec<Settlement>,
}
impl CarrierRecovery for Gate {
    fn validate_carrier(&mut self, effect: CarrierEffectId) -> Result<(), Error> {
        if self.pending == Some(effect) {
            Ok(())
        } else {
            Err(Error::Configuration)
        }
    }
    fn settle_carrier(
        &mut self,
        effect: CarrierEffectId,
        settlement: Settlement,
    ) -> Result<(), Error> {
        self.validate_carrier(effect)?;
        self.settled.push(settlement);
        self.pending = None;
        Ok(())
    }
}
impl Authorization for Gate {
    type Permit = ();
    fn acquire(&mut self, access: Access) -> Result<(), Error> {
        if self.pending.is_some() && access != Access::History {
            return Err(Error::Busy);
        }
        Ok(())
    }
    fn settle(&mut self, _: (), _: Settlement) -> Result<(), Error> {
        Ok(())
    }
    fn acquire_carrier(&mut self, effect: CarrierEffectId) -> Result<(), Error> {
        if self.pending.is_some() || self.paused {
            return Err(Error::Busy);
        }
        self.pending = Some(effect);
        self.admitted += 1;
        Ok(())
    }
}
fn endpoint(cfg: &Provisioned) -> prns_groups_core::Endpoint {
    let binding = cfg.identity.binding();
    prns_groups_core::Endpoint {
        id: prns_groups_core::EndpointId(binding.endpoint),
        incarnation: prns_groups_core::EndpointIncarnation::new(binding.incarnation).unwrap(),
    }
}
fn clock() -> ClockReading {
    ClockReading::Trusted(ClockInterval::new(Timestamp(NOW), Timestamp(NOW)).unwrap())
}
fn execute(
    manager: &mut GroupManager<'_>,
    gate: &mut Gate,
    cfg: &Provisioned,
    command: Command,
) -> Result<NativeReply, Error> {
    let (reply, result) = oneshot::channel();
    manager.execute_authorized(
        Job {
            _input: None,
            command,
            group: Some(cfg.group),
            control: Some(cfg.control),
            reply,
            wake: Arc::new(Notify::new()),
        },
        gate,
    );
    result.blocking_recv().unwrap()
}
fn receive(cfg: &Provisioned, wire: &[u8]) -> Command {
    Command::Receive {
        wire: wire.to_vec(),
        options: SyncOptions {
            incarnation: cfg.incarnation,
            clock: clock(),
            budget: SyncBudget {
                max_requests: 2,
                max_envelopes: 2,
                max_bytes: 1 << 20,
            },
        },
    }
}
fn enrolled_wire(dbs: &[Database; 2]) -> (Provisioned, Vec<u8>) {
    let (mut a, mut b) = (Device::new(&dbs[0], 1), Device::new(&dbs[1], 2));
    let group = a
        .engine
        .create(&mut a.owner, request(1), "carrier", NOW)
        .unwrap();
    let invitation = offer(&mut a, &mut b, group, 2);
    b.engine
        .respond(&mut b.owner, request(3), invitation, true, NOW)
        .unwrap();
    let join = b.work("join");
    let response = a.receive(b.identity, &join, NOW);
    b.complete(&join, &response, NOW);
    let joined = b.work("joined");
    let response = a.receive(b.identity, &joined, NOW);
    b.complete(&joined, &response, NOW);
    let cfg = a.descriptor(group);
    let receiver = b.descriptor(group);
    let request = Request {
        id: RequestId(request(4)),
        original_scope: Some(OriginalScope::Control {
            control: cfg.control,
            incarnation: cfg.incarnation,
        }),
        intent: Intent::Publish(PublicationIntent {
            group,
            namespace: NamespaceId(23),
            schema: SchemaId(1),
            targets: Targets::OtherMembers,
            body: vec![0, 255, 42],
        }),
    };
    assert!(matches!(
        crate::session::execute_with_owner(
            &dbs[0],
            &mut a.owner,
            &cfg,
            Command::Execute {
                request,
                context: ExecutionContext { clock: clock() }
            }
        )
        .unwrap(),
        NativeReply::Outcome(_)
    ));
    let snapshot =
        match crate::session::execute_with_owner(&dbs[0], &mut a.owner, &cfg, Command::Snapshot)
            .unwrap()
        {
            NativeReply::Snapshot(Some(snapshot)) => snapshot,
            _ => panic!("missing inventory"),
        };
    let envelope = match crate::session::execute_with_owner(
        &dbs[0],
        &mut a.owner,
        &cfg,
        Command::Inventory {
            snapshot,
            after: InventoryCursor(0),
            kind: InventoryKind::Record,
            recipient: endpoint(&receiver),
            scan_limit: 100,
        },
    )
    .unwrap()
    {
        NativeReply::Inventory(page) => {
            assert_eq!(page.items.len(), 1);
            page.items[0].id
        }
        _ => panic!("missing inventory page"),
    };
    let wire = match crate::session::execute_with_owner(
        &dbs[0],
        &mut a.owner,
        &cfg,
        Command::Envelope(envelope),
    )
    .unwrap()
    {
        NativeReply::Envelope(Some(wire)) => wire,
        _ => panic!("missing envelope"),
    };
    (receiver, wire)
}

#[test]
fn interrupted_receive_keeps_exact_fences_until_original_ticket_recovery() {
    for cut in [Cut::BeforeReceive, Cut::AfterReceive] {
        let directory = tempfile::tempdir().unwrap();
        let paths = [directory.path().join("a"), directory.path().join("b")];
        let dbs = paths.each_ref().map(|path| Database::create(path).unwrap());
        let (cfg, wire) = enrolled_wire(&dbs);
        let tx = dbs[1].begin_write().unwrap();
        prns_groups_store_redb::change::initialize_in(
            &tx,
            StoreIncarnation::new([71; 24]).unwrap(),
        )
        .unwrap();
        tx.commit().unwrap();
        let mut attachment = Attachment::open(&dbs[1], cfg.local_reticulum_identity).unwrap();
        let mut gate = Gate::default();
        {
            let mut manager = GroupManager::open_attached(&dbs[1], &mut attachment).unwrap();
            fault::set(cut);
            assert!(matches!(
                execute(&mut manager, &mut gate, &cfg, receive(&cfg, &wire)),
                Err(Error::Unavailable)
            ));
            assert!(gate.pending.is_some());
            let before = hash(&std::fs::read(&paths[1]).unwrap());
            for command in [
                Command::Maintain(clock()),
                Command::Config,
                Command::RepairReceipts,
                Command::SaveCursor(endpoint(&cfg), None),
                Command::Execute {
                    request: Request {
                        id: RequestId(request(5)),
                        original_scope: Some(OriginalScope::Control {
                            control: cfg.control,
                            incarnation: cfg.incarnation,
                        }),
                        intent: Intent::Leave { group: cfg.group },
                    },
                    context: ExecutionContext { clock: clock() },
                },
            ] {
                assert!(matches!(
                    execute(&mut manager, &mut gate, &cfg, command),
                    Err(Error::Busy)
                ));
            }
            assert!(matches!(
                execute(&mut manager, &mut gate, &cfg, Command::Snapshot),
                Ok(NativeReply::Snapshot(_))
            ));
            assert_eq!(hash(&std::fs::read(&paths[1]).unwrap()), before);
        }
        assert_eq!(attachment.pending_carrier(), gate.pending);
        gate.paused = true;
        assert!(attachment.recover_carrier(&dbs[1], &mut gate).unwrap());
        assert!(attachment.pending_carrier().is_none());
        assert!(gate.pending.is_none());
        assert_eq!(
            gate.settled,
            vec![match cut {
                Cut::BeforeReceive => Settlement::Rejected,
                Cut::AfterReceive => Settlement::Completed,
            }]
        );
        let mut manager = GroupManager::open_attached(&dbs[1], &mut attachment).unwrap();
        if cut == Cut::BeforeReceive {
            assert!(matches!(
                execute(&mut manager, &mut gate, &cfg, receive(&cfg, &wire)),
                Err(Error::Busy)
            ));
            assert!(gate.pending.is_none());
            assert_eq!(gate.admitted, 1);
            gate.paused = false;
        }
        assert!(matches!(
            execute(&mut manager, &mut gate, &cfg, receive(&cfg, &wire)),
            Ok(NativeReply::Incoming(Incoming::Record(_)))
        ));
        gate.paused = true;
        let admissions = gate.admitted;
        assert!(matches!(
            execute(&mut manager, &mut gate, &cfg, receive(&cfg, &wire)),
            Ok(NativeReply::Incoming(Incoming::Record(_)))
        ));
        assert_eq!(gate.admitted, admissions);
        let page = execute(
            &mut manager,
            &mut gate,
            &cfg,
            Command::Records {
                selection: crate::RecordSelection::All,
                group: cfg.group,
                before: None,
                limit: 10,
                clock: clock(),
            },
        )
        .unwrap();
        let NativeReply::Records(page) = page else {
            panic!("missing history")
        };
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].record.body.as_slice(), &[0, 255, 42]);
    }
}

#[path = "protocol_carrier_tests.rs"]
mod protocol_carrier_tests;
