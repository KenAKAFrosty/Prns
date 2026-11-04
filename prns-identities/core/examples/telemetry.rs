//! A bounded sensor consumer of identity lifecycle, independent of messaging.
//!
//! Run with `cargo run --manifest-path prns-identities/Cargo.toml --example telemetry`.
//! The retained-memory snapshot and key witness are fixtures: reopening a
//! coordinator is demonstrated, not flash durability, cryptography or networking.
//! Production adapters must verify durable key publication and implement scoped
//! activation/quiescence before acknowledging those effects.

use prns_identities_core::*;

const INCARNATION: [u8; 16] = [0x31; 16];
const TELEMETRY: ServiceId = ServiceId(1);
type State = Catalog<1, 6, 1>;

#[derive(Debug)]
struct StaleSnapshot;

/// Retained outside the coordinator, using fixed caller-owned storage.
struct MemorySnapshot(State);

impl Storage<1, 6, 1> for MemorySnapshot {
    type Error = StaleSnapshot;

    fn load(&mut self, into: &mut State) -> Result<(), Self::Error> {
        into.copy_from(&self.0);
        Ok(())
    }

    fn compare_and_store(
        &mut self,
        expected_revision: u64,
        next: &State,
    ) -> Result<(), Self::Error> {
        if self.0.revision() != expected_revision {
            return Err(StaleSnapshot);
        }
        self.0.copy_from(next);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingSample {
    owner: LocalIdentityId,
    public_hash: PublicIdentityHash,
    record_id: [u8; 16],
    temperature_millidegrees: i32,
}

/// Service-owned data survives pausing or replacing its runtime.
#[derive(Default)]
struct Sensor {
    active: Option<IdentityGuard>,
    pending: Option<PendingSample>,
    revision: u64,
}

impl Sensor {
    fn dependencies(&self) -> Result<DependencyReport<1>, Fault> {
        let mut dependency = ServiceDependency::clear(TELEMETRY, self.revision);
        dependency.pending_messages = u32::from(self.pending.is_some());
        DependencyReport::new(&[dependency], 0)
    }
}

fn start(
    owner: &mut Coordinator<'_, 1, 6, 1>,
    storage: &mut MemorySnapshot,
    command: Command<1>,
    tag: u8,
) -> Result<Action<1>, Error<StaleSnapshot>> {
    // Fixture digests only. A real caller supplies its canonical command digest
    // and retains the returned token before requesting any external effects.
    let token = owner.reserve(storage, command, CommandDigest([tag; 32]))?;
    owner.begin(storage, token, &command)?;
    owner.action(token)?.ok_or(Fault::StaleEffect.into())
}

fn activate(
    owner: &mut Coordinator<'_, 1, 6, 1>,
    storage: &mut MemorySnapshot,
    sensor: &mut Sensor,
    fixture_key: VerifiedKey,
    action: Action<1>,
) -> Result<(), Error<StaleSnapshot>> {
    let Action::Activate {
        ticket,
        identity,
        key_reference,
        services,
    } = action
    else {
        return Err(Fault::StaleEffect.into());
    };
    assert_eq!(key_reference, fixture_key.key_reference);
    assert_eq!(identity.public_hash, fixture_key.public_hash);
    assert_eq!(services, ServiceSet::new(&[TELEMETRY])?);
    // This fixture has no listeners or queues: activation restores just the
    // sensor's runtime owner. Real adapters must restore all receive state first.
    sensor.active = Some(identity);
    owner.ack_activated(storage, ticket)
}

fn main() -> Result<(), Error<StaleSnapshot>> {
    let mut storage = MemorySnapshot(State::new(INCARNATION));
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut sensor = Sensor::default();

    // Independent infrastructure handles belong to their own owners. They are
    // never handed to the sensor's lifecycle effects or treated as persona keys.
    let management_key = KeyReference([0xA1; 32]);
    let transport_key = KeyReference([0xA2; 32]);
    let infrastructure_before = (management_key, transport_key);

    let (identity, fixture_key, original_sample, original_admission) = {
        let mut owner = Coordinator::open(&mut storage, &mut working, &mut staging)?;
        let create = Command::Create(CreateRequest {
            source: KeySource::Generate,
            metadata: Metadata::new("Greenhouse sensor", None)?,
            services: ServiceSet::new(&[TELEMETRY])?,
        });
        let Action::PublishKey {
            ticket,
            key_reference,
            ..
        } = start(&mut owner, &mut storage, create, 1)?
        else {
            return Err(Fault::StaleEffect.into());
        };
        assert!(owner.snapshot().identities().next().is_none());

        // Stand-in for an idempotent vault write followed by verified durable
        // readback. These bytes are neither a generated key nor a real hash.
        let fixture_key = VerifiedKey {
            key_reference,
            public_hash: PublicIdentityHash([0x51; 16]),
        };
        let identity = ticket.identity;
        assert_eq!(
            owner.ack_key(&mut storage, ticket, fixture_key)?,
            OperationResult::Created { identity }
        );
        assert_eq!(owner.identity(identity)?.desired, DesiredState::Paused);

        let enable = owner.prepare(identity, TransitionTarget::Enable, sensor.dependencies()?)?;
        let action = start(&mut owner, &mut storage, Command::Transition(enable), 2)?;
        activate(&mut owner, &mut storage, &mut sensor, fixture_key, action)?;

        let admission = owner.admit(identity, fixture_key.public_hash, AdmissionClass::Network)?;
        owner.validate_admission(admission)?;
        let sample = PendingSample {
            owner: admission.identity.id,
            public_hash: admission.identity.public_hash,
            record_id: [0x61; 16],
            temperature_millidegrees: 23_500,
        };
        sensor.pending = Some(sample);
        sensor.revision += 1;

        let pause = owner.prepare(identity, TransitionTarget::Pause, sensor.dependencies()?)?;
        let stop = owner.apply_stop(&mut storage, pause, CommandDigest([3; 32]))?;
        let Action::Quiesce {
            ticket,
            identity: captured,
            ..
        } = owner.action(stop)?.ok_or(Fault::StaleEffect)?
        else {
            return Err(Fault::StaleEffect.into());
        };
        assert_eq!(captured.id, sample.owner);
        // Only the sensor runtime stops; its accepted sample remains retained.
        sensor.active = None;
        assert_eq!(
            owner.ack_quiesced(&mut storage, ticket, sensor.dependencies()?)?,
            OperationResult::Paused { identity }
        );
        assert_eq!(sensor.pending, Some(sample));
        (identity, fixture_key, sample, admission)
    };

    // Reopening the coordinator does not implicitly reactivate a paused owner.
    let mut owner = Coordinator::open(&mut storage, &mut working, &mut staging)?;
    assert!(owner.recovery_action(identity)?.is_none());
    assert_eq!(
        owner.admit(identity, fixture_key.public_hash, AdmissionClass::Network),
        Err(Fault::Paused)
    );
    let enable = owner.prepare(identity, TransitionTarget::Enable, sensor.dependencies()?)?;
    let action = start(&mut owner, &mut storage, Command::Transition(enable), 4)?;
    activate(&mut owner, &mut storage, &mut sensor, fixture_key, action)?;

    // Retry obtains fresh runtime admission for the original durable owner;
    // it does not use an old session guard or invent a replacement record ID.
    assert_eq!(
        owner.validate_admission(original_admission),
        Err(Fault::StaleSession)
    );
    let retry = owner.admit(
        original_sample.owner,
        original_sample.public_hash,
        AdmissionClass::Network,
    )?;
    owner.validate_admission(retry)?;
    assert_eq!(sensor.active, Some(retry.identity));
    assert_eq!(sensor.pending, Some(original_sample));
    assert_eq!(infrastructure_before, (management_key, transport_key));

    println!("Sensor reactivated with the original pending sample and owner; infrastructure handles unchanged.");
    println!("Retained-memory lifecycle example only; no message was sent or acknowledged.");
    println!(
        "Host layout for 1 identity, 6 operation slots, 1 service: {} bytes per catalog buffer; {} bytes for the coordinator.",
        core::mem::size_of::<State>(),
        core::mem::size_of::<Coordinator<'_, 1, 6, 1>>()
    );
    Ok(())
}
