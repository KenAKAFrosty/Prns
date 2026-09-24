"""Exercise standalone pairing/access through actual generated UniFFI Python.

This desktop test uses two persisted hosts and public SDK methods only. It
qualifies generated lifting/lowering and ownership, not a mobile JavaScript VM.
"""
import asyncio
import socket
import tempfile
from pathlib import Path

from foreign_conformance import config, execute, p

RC = p.RemoteControlNativeRemoteControlEvent


def remote_control(root):
    return p.RemoteControlNativeRemoteControlConfig(
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


async def run():
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
        target_events, controller_events = target.application_events(), controller.application_events()
        try:
            await connect(target, controller)
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
            available = await next_rc(controller_events, RC.PAIRING_AVAILABLE)
            assert available.endpoint == opened.endpoint
            assert available.public_app_data == b'foreign target'
            assert available.observed_at.value < available.expires_at.value
            link = await execute(controller, p.HostCommand.ESTABLISH_LINK(available.endpoint.destination_hash.value),
                                 p.CommandOutcome.LINK_ESTABLISHED)
            await completed(controller.begin_remote_control_controller_pairing(p.RemoteControlBeginRemoteControlControllerPairing(
                context=p.RemoteControlPairingContext(endpoint=available.endpoint,
                                                     link_id=p.RemoteControlLinkId(value=link.link_id)),
                invitation_code=opened.invitation_code,
                pairing_expires_at=available.expires_at,
            )), p.RemoteControlBeginRemoteControlControllerPairingSettlement)
            target_view = (await next_rc(target_events, RC.TARGET_CONFIRMATION_REQUIRED)).confirmation
            controller_view = (await next_rc(controller_events, RC.CONTROLLER_CONFIRMATION_REQUIRED)).confirmation
            assert target_view.attempt_id == controller_view.attempt_id
            assert target_view.confirmation_code == controller_view.confirmation_code
            assert target_view.permissions == permissions
            attempt_id, target_identity = controller_view.attempt_id, controller_view.target
            # Public views and a foreign stream lease own no live pairing authority.
            del target_view, controller_view
            controller_events.close_stream()
            controller_events = controller.application_events()
            await completed(target.approve_remote_control_target_pairing(
                p.RemoteControlApproveRemoteControlTargetPairing(attempt_id=attempt_id)),
                p.RemoteControlApproveRemoteControlTargetPairingSettlement)
            await completed(controller.approve_remote_control_controller_pairing(
                p.RemoteControlApproveRemoteControlControllerPairing(attempt_id=attempt_id)),
                p.RemoteControlApproveRemoteControlControllerPairingSettlement)
            persisted = await next_rc(controller_events, RC.CONTROLLER_AUTHORIZATION_PERSISTED)
            assert persisted.attempt_id == attempt_id
            target_persisted = await next_rc(target_events, RC.TARGET_AUTHORIZATION_PERSISTED)
            assert target_persisted.attempt_id == attempt_id
            inventory = await completed(controller.remote_control_target_inventory(), p.RemoteControlTargetInventorySettlement)
            assert len(inventory.targets) == 1
            identity_hash = inventory.targets[0]
            resolved = await completed(controller.resolve_remote_control_target(identity_hash),
                                       p.RemoteControlResolveRemoteControlTargetSettlement)
            assert resolved.target == identity_hash
            assert resolved.permitted_requests == permissions.permitted_requests
            await completed(controller.set_remote_control_target_access(p.RemoteControlTargetAccess(
                target=target_identity, authority=permissions.authority,
                permitted_requests=permissions.permitted_requests)), p.RemoteControlSetRemoteControlTargetAccessSettlement)
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
        restored = await p.open_host_with_remote_control(controller_config, controller_rc)
        try:
            client = restored.client()
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
        finally:
            await restored.stop()
    print('UniFFI RemoteControl conformance passed: discovery, confirmation lease replacement, pairing, all 11 controls, durable authorization, and durable forget.')


if __name__ == '__main__':
    asyncio.run(asyncio.wait_for(run(), 60))
