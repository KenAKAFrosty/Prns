#![allow(clippy::unwrap_used, clippy::panic)]

extern crate alloc;

mod enrollment;
mod properties;
mod records;

use super::*;
use crate::domain_primitives::{DeviceId, Enrollment, EnrollmentFailure};
use pipecircuit::StateMachine;
use prns_core::{
    crypto::{Ed25519PublicKey, X25519PublicKey},
    identity::{IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey},
    remote_control::{RemoteControlPairingAttemptId, RemoteControlTargetIdentity},
};

fn registry(capacity: u32) -> DeviceRegistry {
    DeviceRegistry::try_new(NonZeroU32::new(capacity).unwrap()).unwrap()
}

fn target(seed: u8) -> RemoteControlTargetIdentity {
    RemoteControlTargetIdentity::new(IdentityPublicKeys {
        encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([seed; 32])),
        signing: IdentitySigningPublicKey::new(Ed25519PublicKey([seed; 32])),
    })
}

fn create(registry: &mut DeviceRegistry, name: &str) -> DeviceId {
    let outcome = registry.step(CreateDevice {
        label: DeviceLabel::new(name).unwrap(),
    });
    let CreateDeviceOutcome::Created { device } = outcome else {
        panic!("unexpected creation: {outcome:?}");
    };
    device
}

fn read(registry: &mut DeviceRegistry, device: DeviceId) -> DeviceSnapshot {
    let outcome = registry.step(ReadDevice { device });
    let ReadDeviceOutcome::Found { device } = outcome else {
        panic!("unexpected read: {outcome:?}");
    };
    device
}

fn list<const CAPACITY: usize>(registry: &mut DeviceRegistry) -> heapless::Vec<DeviceId, CAPACITY> {
    let outcome = registry.step(ListDevices::<CAPACITY>);
    let ListDevicesOutcome::Listed { devices } = outcome else {
        panic!("unexpected listing: {outcome:?}");
    };
    devices
}

fn begin_input(device: DeviceId, seed: u8) -> BeginEnrollment {
    BeginEnrollment {
        device,
        attempt: RemoteControlPairingAttemptId::from_test_transcript_digest_bytes([seed; 32]),
        target: target(seed),
    }
}

fn begin(registry: &mut DeviceRegistry, device: DeviceId, seed: u8) -> Enrollment {
    let outcome = registry.step(begin_input(device, seed));
    let BeginEnrollmentOutcome::Started { enrollment } = outcome else {
        panic!("unexpected enrollment: {outcome:?}");
    };
    enrollment
}

fn complete(enrollment: Enrollment, seed: u8) -> CompleteEnrollment {
    CompleteEnrollment {
        enrollment,
        target: target(seed),
    }
}
