"""Exercise standalone pairing/access through actual generated UniFFI Python.

This desktop test uses two persisted hosts and public SDK methods only. It
qualifies generated lifting/lowering and ownership, not a mobile JavaScript VM.
"""
import asyncio
import socket
import tempfile
from pathlib import Path

from foreign_conformance import config, execute, p

RC = p.RemoteControlHostRemoteControlEvent


def remote_control(root):
    return p.RemoteControlHostRemoteControlConfig(
        controller_identity=p.IdentityConfig.LOAD_OR_CREATE(str(root / 'controller-identity')),
        target_identity=p.IdentityConfig.LOAD_OR_CREATE(str(root / 'target-identity')),
        initial_controller_grants=[],
        self_announcement=p.RemoteControlSelfAnnouncement.UNAVAILABLE(),
        capabilities=p.RemoteControlCapabilities(requests=p.RemoteControlRequestSet(
            kinds=[p.RemoteControlRequestKind.DESCRIBE])),
    )


async def completed(operation, settlement_type):
    result = await operation
    assert isinstance(result, settlement_type.COMPLETED), result
    return result.value


async def next_rc(stream, expected):
    async with asyncio.timeout(15):
        while await stream.ready():
            value = stream.try_next()
            if isinstance(value, p.HostEvent.REMOTE_CONTROL) and isinstance(value.event, expected):
                return value.event
    raise AssertionError('stream ended before the required RemoteControl observation')


async def connect(target, controller):
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    address = f'127.0.0.1:{port}'
    await execute(target, p.HostCommand.ATTACH_TCP_SERVER(address, p.Bitrate.AUTO()),
                  p.CommandOutcome.INTERFACE_ATTACHED)
    attached = await execute(controller, p.HostCommand.ATTACH_TCP_CLIENT(address, p.Bitrate.AUTO()),
                             p.CommandOutcome.INTERFACE_ATTACHED)
    async with asyncio.timeout(15):
        while not any(item.interface_id == attached.interface and item.health == p.InterfaceHealth.CONNECTED
                      for item in (await controller.snapshot()).interfaces):
            await asyncio.sleep(.01)


def radio_and_policy_models():
    # These are generated records of the actual core models. No compact profile
    # grammar or administrator-kind list belongs to a foreign consumer.
    region = p.RemoteControlSubGRegion.REGULATED(p.RemoteControlRegulatoryRegion.US915)
    bandwidth = next(value for value in p.RemoteControlLoraBandwidth
                     if value.name.replace('_', '') == 'BW125KHZ')
    modulation = p.RemoteControlModulation.LORA(
        p.RemoteControlSpreadingFactor.SF7, bandwidth, p.RemoteControlCodingRate.CR45)
    result = p.remote_control_radio_profile(region, p.RemoteControlFrequency(value=915_000_000),
        modulation, p.RemoteControlTxPower(value=10), p.RemoteControlPreambleSymbols(value=8))
    assert isinstance(result, p.RemoteControlRadioProfileSettlement.COMPLETED), result
    band = p.RemoteControlBandProfile.SUB_G(result.value)
    wire = p.remote_control_lora_profile_from_band(band)
    assert wire is not None and p.remote_control_lora_profile_band(wire) == band
    failed = p.remote_control_radio_profile(region, p.RemoteControlFrequency(value=868_000_000),
        modulation, p.RemoteControlTxPower(value=10), p.RemoteControlPreambleSymbols(value=8))
    assert isinstance(failed, p.RemoteControlRadioProfileSettlement.FAILED)
    assert isinstance(failed.failure, p.RemoteControlRadioProfileError.NOMINAL_CHANNEL_OUTSIDE_REGION)
    assert failed.failure.region == region and failed.failure.center_hz == 868_000_000
    invalid = p.RemoteControlRadioProfile(frequency=result.value.frequency, modulation=modulation,
        tx_power=result.value.tx_power, preamble=p.RemoteControlPreambleSymbols(value=0), region=region)
    try:
        p.remote_control_lora_profile_from_band(p.RemoteControlBandProfile.SUB_G(invalid))
        raise AssertionError('private checked radio constructor bypassed')
    except p.BindingError.InvalidInput as error:
        assert error.field == 'RadioProfile' and 'EmptyPreamble' in error.detail

    bandwidth24 = next(value for value in p.RemoteControlLoraBandwidth
                       if value.name.replace('_', '') == 'BW812KHZ')
    modulation24 = p.RemoteControlModulation.LORA(
        p.RemoteControlSpreadingFactor.SF7, bandwidth24, p.RemoteControlCodingRate.CR45)
    result24 = p.remote_control_ghz24_profile(p.RemoteControlFrequency(value=2_445_000_000),
        modulation24, p.RemoteControlTxPower(value=10), p.RemoteControlPreambleSymbols(value=18))
    assert isinstance(result24, p.RemoteControlGhz24ProfileSettlement.COMPLETED), result24
    band24 = p.RemoteControlBandProfile.GHZ24(result24.value)
    wire24 = p.remote_control_lora_profile_from_band(band24)
    assert wire24 is not None and p.remote_control_lora_profile_band(wire24) == band24
    failed24 = p.remote_control_ghz24_profile(p.RemoteControlFrequency(value=2_445_000_000),
        modulation24, p.RemoteControlTxPower(value=14), p.RemoteControlPreambleSymbols(value=18))
    assert isinstance(failed24, p.RemoteControlGhz24ProfileSettlement.FAILED)
    assert failed24.failure == p.RemoteControlGhz24ProfileError.TRANSMIT_POWER_OUTSIDE_RANGE
    assert p.remote_control_request_requires_administrator(p.RemoteControlRequestKind.AUTHORIZE_CONTROLLER)
    assert not p.remote_control_request_requires_administrator(p.RemoteControlRequestKind.DESCRIBE)


async def run():
    radio_and_policy_models()
    with tempfile.TemporaryDirectory(prefix='prns-uniffi-pairing-') as temporary:
        root = Path(temporary)
        target_root, controller_root = root / 'target', root / 'controller'
        target_root.mkdir()
        controller_root.mkdir()
        target_config, controller_config = config(target_root, []), config(controller_root, [])
        target_rc, controller_rc = remote_control(target_root), remote_control(controller_root)
        target_owner = await p.open_host_with_remote_control(target_config, target_rc)
        controller_owner = await p.open_host_with_remote_control(controller_config, controller_rc)
        target, controller = target_owner.client(), controller_owner.client()
        retained = p.RemoteControlOperations(1)
        target_events, controller_events = target.application_events(), controller.application_events()
        try:
            controller_identity = await completed(controller.remote_control_controller_identity(),
                                                  p.RemoteControlControllerIdentitySettlement)
            wrong_recipient = await completed(target.remote_control_controller_identity(),
                                              p.RemoteControlControllerIdentitySettlement)
            controller_card = p.RemoteControlControllerCard(controller=controller_identity)
            encoded = p.encode_remote_control_controller_card(controller_card)
            assert isinstance(encoded, p.RemoteControlEncodeRemoteControlControllerCardSettlement.COMPLETED)
            decoded = p.decode_remote_control_controller_card(encoded.value)
            assert isinstance(decoded, p.RemoteControlDecodeRemoteControlControllerCardSettlement.COMPLETED)
            assert decoded.value == controller_card
            malformed = p.decode_remote_control_target_access_card('not-a-card')
            assert isinstance(malformed, p.RemoteControlDecodeRemoteControlTargetAccessCardSettlement.FAILED)
            assert isinstance(malformed.failure, p.RemoteControlPublicCardError.INVALID_TEXT_ENVELOPE)
            assert (await completed(controller.remote_control_target_inventory(),
                                    p.RemoteControlTargetInventorySettlement)).targets == []
            await connect(target, controller)
            snapshot_settlement = p.RemoteControlControllerPairingSnapshotSettlement
            snapshot_cases = p.RemoteControlControllerPairingSnapshot
            current = await completed(controller.remote_control_controller_pairing_snapshot(), snapshot_settlement)
            assert isinstance(current, snapshot_cases.IDLE)
            permissions = p.RemoteControlPairingPermissions(
                authority=p.RemoteControlControllerAuthority.OPERATOR,
                permitted_requests=p.RemoteControlRequestSet(kinds=[p.RemoteControlRequestKind.DESCRIBE]),
            )
            opened = await completed(target.open_remote_control_pairing(p.RemoteControlOpenRemoteControlPairing(
                target=p.RemoteControlEgressTarget.ALL_INTERFACES(),
                expires_after=p.RemoteControlPairingExpiresAfter(value=60_000),
                attempt_timeout=p.RemoteControlPairingAttemptTimeout(value=30_000),
                permissions=permissions,
                public_app_data=p.RemoteControlPairingPublicAppDataBytes(value=b'foreign target'),
            )), p.RemoteControlOpenRemoteControlPairingSettlement)
            discovery_settlement = p.RemoteControlPairingDiscoverySettlement
            async with asyncio.timeout(15):
                while True:
                    discovery = await completed(controller.remote_control_pairing_discovery(), discovery_settlement)
                    if discovery.observations:
                        break
                    await asyncio.sleep(.01)
            assert discovery.capacity == 64 and discovery.evicted == 0
            available = discovery.observations[0]
            assert available.endpoint == opened.endpoint
            assert available.public_app_data.value == b'foreign target'
            assert available.observed_at.value <= discovery.observed_at.value < available.expires_at.value
            # Querying did not consume the existing queue observation or lease.
            queued = await next_rc(controller_events, RC.PAIRING_AVAILABLE)
            assert queued.endpoint == available.endpoint
            assert queued.observed_at == available.observed_at
            await completed(controller.initiate_remote_control_controller_pairing(p.RemoteControlInitiateRemoteControlControllerPairing(
                endpoint=available.endpoint,
                invitation_code=opened.invitation_code,
                expires_at=available.expires_at,
            )), p.RemoteControlInitiateRemoteControlControllerPairingSettlement)
            target_view = (await next_rc(target_events, RC.TARGET_CONFIRMATION_REQUIRED)).confirmation
            controller_view = (await next_rc(controller_events, RC.CONTROLLER_CONFIRMATION_REQUIRED)).confirmation
            assert target_view.attempt_id == controller_view.attempt_id
            assert target_view.confirmation_code == controller_view.confirmation_code
            assert p.remote_control_pairing_confirmation_code(controller_view.attempt_id) == controller_view.confirmation_code
            assert p.remote_control_pairing_confirmation_code(p.RemoteControlPairingAttemptId(value=bytes(32))) == '000000'

            assert target_view.permissions == permissions
            attempt_id, target_identity = controller_view.attempt_id, controller_view.target
            # Public views and a foreign stream lease own no live pairing authority.
            del target_view, controller_view
            controller_events.close_stream()
            controller_events = controller.application_events()
            # Recover current confirmation directly from engine authority after
            # consuming and releasing the event views and stream lease. No UI
            # event replay or app-owned pairing state is required.
            current = await completed(controller.remote_control_controller_pairing_snapshot(), snapshot_settlement)
            assert isinstance(current, snapshot_cases.AWAITING_APPROVAL)
            assert current.confirmation.confirmation.attempt_id == attempt_id
            assert current.confirmation.confirmation.permissions == permissions
            assert current.confirmation.confirmation.target == target_identity
            assert current.confirmation.window.expires_at.value > 0
            await completed(target.approve_remote_control_target_pairing(
                p.RemoteControlApproveRemoteControlTargetPairing(attempt_id=attempt_id)),
                p.RemoteControlApproveRemoteControlTargetPairingSettlement)
            await completed(controller.approve_remote_control_controller_pairing(
                p.RemoteControlApproveRemoteControlControllerPairing(attempt_id=attempt_id)),
                p.RemoteControlApproveRemoteControlControllerPairingSettlement)
            persisted = await next_rc(controller_events, RC.CONTROLLER_AUTHORIZATION_PERSISTED)
            assert persisted.attempt_id == attempt_id
            current = await completed(controller.remote_control_controller_pairing_snapshot(), snapshot_settlement)
            assert isinstance(current, snapshot_cases.IDLE)
            target_persisted = await next_rc(target_events, RC.TARGET_AUTHORIZATION_PERSISTED)
            assert target_persisted.attempt_id == attempt_id
            inventory = await completed(controller.remote_control_target_inventory(), p.RemoteControlTargetInventorySettlement)
            assert len(inventory.targets) == 1
            identity_hash = inventory.targets[0]
            resolved = await completed(controller.resolve_remote_control_target(identity_hash),
                                       p.RemoteControlResolveRemoteControlTargetSettlement)
            assert resolved.target == identity_hash
            assert p.remote_control_target_identity_hash(target_identity) == identity_hash
            assert p.remote_control_controller_identity_hash(controller_identity) == p.remote_control_controller_identity_hash(resolved.controller)

            assert resolved.permitted_requests == permissions.permitted_requests
            await completed(controller.set_remote_control_target_access(p.RemoteControlTargetAccess(
                target=target_identity, authority=permissions.authority,
                permitted_requests=permissions.permitted_requests)), p.RemoteControlSetRemoteControlTargetAccessSettlement)
            access = p.RemoteControlTargetAccess(target=target_identity, authority=permissions.authority,
                                                 permitted_requests=permissions.permitted_requests)
            card = p.RemoteControlTargetAccessCard(recipient=controller_identity, access=access)
            exported = p.encode_remote_control_target_access_card(card)
            assert isinstance(exported, p.RemoteControlEncodeRemoteControlTargetAccessCardSettlement.COMPLETED)
            decoded = p.decode_remote_control_target_access_card(exported.value)
            assert isinstance(decoded, p.RemoteControlDecodeRemoteControlTargetAccessCardSettlement.COMPLETED)
            assert decoded.value == card
            card = decoded.value
            # The public card makes no trust assertion. This fixture explicitly
            # chooses its trusted, locally constructed source before importing it.
            await completed(controller.save_remote_control_target_access_card(card),
                                       p.RemoteControlSaveRemoteControlTargetAccessCardSettlement)
            try:
                await controller.save_remote_control_target_access_card(p.RemoteControlTargetAccessCard(
                    recipient=p.RemoteControlControllerIdentity(public_keys=bytes(63)), access=access))
                raise AssertionError('malformed public key admitted')
            except p.BindingError.InvalidInput:
                pass
            mismatch = await controller.save_remote_control_target_access_card(
                p.RemoteControlTargetAccessCard(recipient=wrong_recipient, access=access))
            assert isinstance(mismatch, p.RemoteControlSaveRemoteControlTargetAccessCardSettlement.FAILED)
            assert mismatch.failure == p.RemoteControlSaveRemoteControlTargetAccessError.RECIPIENT_MISMATCH
            conflict = await controller.save_remote_control_target_access_card(p.RemoteControlTargetAccessCard(
                recipient=controller_identity,
                access=p.RemoteControlTargetAccess(target=target_identity,
                    authority=p.RemoteControlControllerAuthority.ADMINISTRATOR,
                    permitted_requests=permissions.permitted_requests)))
            assert isinstance(conflict, p.RemoteControlSaveRemoteControlTargetAccessCardSettlement.FAILED)
            assert conflict.failure == p.RemoteControlSaveRemoteControlTargetAccessError.CONFLICT
            assert (await completed(controller.remote_control_target_inventory(),
                                    p.RemoteControlTargetInventorySettlement)).targets == [identity_hash]
            # Pairing discovery is a distinct endpoint. Announce the canonical
            # management endpoint before testing ordinary target connection.
            management_destination = resolved.endpoint.destination_hash.value
            await execute(target, p.HostCommand.ANNOUNCE(management_destination, None), p.CommandOutcome.ANNOUNCED)
            async with asyncio.timeout(15):
                while not any(route.destination == management_destination for route in (await controller.snapshot()).routes):
                    await asyncio.sleep(.01)
            operation = retained.submit_target(controller, identity_hash.value, p.RemoteControlRequest.DESCRIBE())
            waiter = asyncio.create_task(retained.wait(operation))
            await asyncio.sleep(0)
            waiter.cancel()
            try:
                await waiter
            except asyncio.CancelledError:
                pass
            async with asyncio.timeout(15):
                snapshot = await retained.wait(operation)
            assert isinstance(snapshot.state, p.RemoteControlOperationState.COMPLETED), snapshot.state
            assert isinstance(snapshot.state.settlement, p.RemoteControlExchangeSettlement.COMPLETED)
            assert isinstance(snapshot.state.settlement.response, p.RemoteControlResponse.DESCRIBE)
            assert snapshot.request_kind == p.RemoteControlRequestKind.DESCRIBE
            assert retained.operation(operation) == snapshot
            assert retained.operations() == [snapshot]
            try:
                retained.submit_target(controller, identity_hash.value, p.RemoteControlRequest.DESCRIBE())
                raise AssertionError('full retention must not evict or resubmit')
            except p.BindingError.Busy:
                pass
            # Foreign-supplied attempt IDs are validated against the bounded engine state.
            missing = p.RemoteControlPairingAttemptId(value=bytes(32))
            rejected = await controller.reject_remote_control_controller_pairing(
                p.RemoteControlRejectRemoteControlControllerPairing(attempt_id=missing))
            assert isinstance(rejected, p.RemoteControlRejectRemoteControlControllerPairingSettlement.FAILED)
            rejected = await target.reject_remote_control_target_pairing(
                p.RemoteControlRejectRemoteControlTargetPairing(attempt_id=missing))
            assert isinstance(rejected, p.RemoteControlRejectRemoteControlTargetPairingSettlement.FAILED)
            await completed(target.close_remote_control_pairing(), p.RemoteControlCloseRemoteControlPairingSettlement)
        finally:
            target_events.close_stream()
            controller_events.close_stream()
            await asyncio.gather(target_owner.stop(), controller_owner.stop(), controller_owner.stop())
        assert await retained.wait(operation) == snapshot, 'joined host stop preserves native result'
        try:
            await controller.remote_control_pairing_discovery()
            raise AssertionError('stopped generation returned discovery')
        except p.BindingError.Stopped:
            pass
        restored = await p.open_host_with_remote_control(controller_config, controller_rc)
        try:
            client = restored.client()
            reopened_discovery = await completed(client.remote_control_pairing_discovery(), discovery_settlement)
            assert reopened_discovery.generation != discovery.generation
            assert reopened_discovery.observations == []
            assert retained.operation(operation) == snapshot, 'a new host does not replay or replace results'
            assert retained.retire(operation) == p.RemoteControlOperationRetirement.RETIRED
            assert await retained.wait(operation) is None
            inventory = await completed(client.remote_control_target_inventory(), p.RemoteControlTargetInventorySettlement)
            assert inventory.targets == [identity_hash]
            resolved = await completed(client.resolve_remote_control_target(identity_hash),
                                       p.RemoteControlResolveRemoteControlTargetSettlement)
            assert resolved.permitted_requests == permissions.permitted_requests
            await completed(client.forget_remote_control_target(identity_hash), p.RemoteControlForgetRemoteControlTargetSettlement)
            inventory = await completed(client.remote_control_target_inventory(), p.RemoteControlTargetInventorySettlement)
            assert inventory.targets == []
        finally:
            await restored.stop()
        restored = await p.open_host_with_remote_control(controller_config, controller_rc)
        try:
            inventory = await completed(restored.client().remote_control_target_inventory(), p.RemoteControlTargetInventorySettlement)
            assert inventory.targets == [], 'forgotten authorization must remain absent after restart'
            # Import into an actually empty reopened store, then reopen again:
            # success must be durable before the new access becomes active.
            await completed(restored.client().save_remote_control_target_access_card(card),
                            p.RemoteControlSaveRemoteControlTargetAccessCardSettlement)

        finally:
            await restored.stop()
        restored = await p.open_host_with_remote_control(controller_config, controller_rc)
        try:
            inventory = await completed(restored.client().remote_control_target_inventory(), p.RemoteControlTargetInventorySettlement)
            assert inventory.targets == [identity_hash], 'imported access must survive restart'
            resolved = await completed(restored.client().resolve_remote_control_target(identity_hash),
                                       p.RemoteControlResolveRemoteControlTargetSettlement)
            assert resolved.permitted_requests == permissions.permitted_requests, 'failed conflict import must preserve access'
        finally:
            await restored.stop()
    print('UniFFI RemoteControl conformance passed: discovery, confirmation lease replacement, pairing, canonical card codecs, recipient/conflict preservation, durable import/reopen, durable authorization, durable forget, and bounded waiter-independent canonical exchanges.')


if __name__ == '__main__':
    asyncio.run(asyncio.wait_for(run(), 60))
