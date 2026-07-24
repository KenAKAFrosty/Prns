from __future__ import annotations

import asyncio
import ctypes
import threading
from dataclasses import dataclass
from functools import wraps
from typing import AsyncIterator, Generic, TypeVar

from . import generated as g
from ._native import (
    ByteView,
    CommandResult,
    ContractInfo,
    DestinationConfig as NativeDestinationConfig,
    DestinationName as NativeDestinationName,
    HostOptions as NativeHostOptions,
    IdentityConfig as NativeIdentityConfig,
    Lifecycle,
    Limits as NativeLimits,
    NativeLibrary,
    StringView,
    bytes_from_view,
)

NEVER_TIMEOUT = 2**32 - 1
T = TypeVar("T")


def _daemon_future(function):
    loop = asyncio.get_running_loop()
    future = loop.create_future()
    done = threading.Event()

    def complete(result=None, error=None):
        if future.done():
            return
        if error is None:
            future.set_result(result)
        else:
            future.set_exception(error)

    def run():
        try:
            result = function()
        except BaseException as error:
            loop.call_soon_threadsafe(complete, None, error)
        else:
            loop.call_soon_threadsafe(complete, result, None)
        finally:
            done.set()

    threading.Thread(target=run, daemon=True).start()
    return future, done


async def _await_daemon(future):
    while True:
        try:
            return await asyncio.wait_for(asyncio.shield(future), 0.1)
        except asyncio.TimeoutError:
            continue


def _consume_future(future):
    if future is not None and future.done() and not future.cancelled():
        future.exception()


class PrnsError(Exception):
    def __init__(self, status: g.Status):
        self.status = status
        super().__init__(f"Personal RNS host operation failed with {status.name}")


class ContractMismatchError(PrnsError):
    def __init__(
        self,
        actual_abi: int,
        actual_schema: int,
        actual_version: str,
    ):
        self.actual_abi = actual_abi
        self.actual_schema = actual_schema
        self.actual_version = actual_version
        super().__init__(g.Status.CONTRACT_MISMATCH)


def _status(value: int) -> g.Status:
    try:
        return g.Status(value)
    except ValueError as error:
        raise RuntimeError(f"unknown Personal RNS status {value}") from error


def _check(value: int) -> None:
    status = _status(value)
    if status is not g.Status.OK:
        raise PrnsError(status)


class _Arena:
    def __init__(self):
        self.keepalive: list[object] = []

    def __enter__(self) -> _Arena:
        return self

    def __exit__(self, _type, _value, _traceback) -> None:
        self.close()

    def __del__(self):
        self.close()

    def close(self) -> None:
        for value in reversed(self.keepalive):
            try:
                ctypes.memset(
                    ctypes.addressof(value),
                    0,
                    ctypes.sizeof(value),
                )
            except (TypeError, ValueError):
                pass
        self.keepalive.clear()

    def bytes(self, value: bytes | bytearray | memoryview) -> ByteView:
        view = memoryview(value).cast("B")
        if not view:
            return ByteView()
        buffer = (ctypes.c_uint8 * len(view)).from_buffer_copy(view)
        self.keepalive.append(buffer)
        return ByteView(ctypes.cast(buffer, ctypes.POINTER(ctypes.c_uint8)), len(view))

    def string(self, value: str) -> StringView:
        view = self.bytes(value.encode())
        return StringView(view.data, view.length)

    def array(self, item_type, values):
        values = tuple(values)
        if not values:
            return ctypes.POINTER(item_type)()
        array = (item_type * len(values))(*values)
        self.keepalive.append(array)
        return ctypes.cast(array, ctypes.POINTER(item_type))


@dataclass(frozen=True, slots=True)
class HostLimits:
    pending_commands: int
    application_events: int
    retained_event_bytes: int
    diagnostics: int

    @classmethod
    def balanced(cls) -> HostLimits:
        return cls(
            g.BALANCED_PENDING_COMMANDS,
            g.BALANCED_APPLICATION_EVENTS,
            g.BALANCED_RETAINED_EVENT_BYTES,
            g.BALANCED_DIAGNOSTICS,
        )


@dataclass(frozen=True, slots=True)
class HostOptions:
    identity: g.IdentityConfig
    role: g.HostRole
    destinations: tuple[g.DestinationConfig, ...] = ()
    required_capabilities: tuple[g.Capability, ...] = ()
    limits: HostLimits = HostLimits(
        g.BALANCED_PENDING_COMMANDS,
        g.BALANCED_APPLICATION_EVENTS,
        g.BALANCED_RETAINED_EVENT_BYTES,
        g.BALANCED_DIAGNOSTICS,
    )

    @classmethod
    def endpoint(
        cls,
        identity: g.IdentityConfig,
        destinations: tuple[g.DestinationConfig, ...] = (),
        required_capabilities: tuple[g.Capability, ...] = (),
        limits: HostLimits | None = None,
    ) -> HostOptions:
        return cls(
            identity,
            g.HostRole.ENDPOINT,
            destinations,
            required_capabilities,
            limits or HostLimits.balanced(),
        )

    @classmethod
    def transport(
        cls,
        identity: g.IdentityConfig,
        destinations: tuple[g.DestinationConfig, ...] = (),
        required_capabilities: tuple[g.Capability, ...] = (),
        limits: HostLimits | None = None,
    ) -> HostOptions:
        return cls(
            identity,
            g.HostRole.TRANSPORT,
            destinations,
            required_capabilities,
            limits or HostLimits.balanced(),
        )


@dataclass(frozen=True, slots=True)
class LifecycleSnapshot:
    revision: int
    phase: g.LifecyclePhase
    stop_reason: g.StopReason | None


@dataclass(frozen=True, slots=True)
class CommandSucceeded:
    outcome: g.CommandOutcome


@dataclass(frozen=True, slots=True)
class CommandFailed:
    failure: g.CommandFailureKind
    detail: str


CommandSettlement = CommandSucceeded | CommandFailed


@dataclass(frozen=True, slots=True)
class StreamClaimed(Generic[T]):
    stream: T


@dataclass(frozen=True, slots=True)
class StreamAlreadyClaimed:
    lane: str


StreamClaim = StreamClaimed[T] | StreamAlreadyClaimed


def _host_operation(function):
    @wraps(function)
    def invoke(host, *args, **kwargs):
        with host._lock:
            host._require_open()
            return function(host, *args, **kwargs)

    return invoke


def _marshal_identity(identity: g.IdentityConfig, arena: _Arena) -> NativeIdentityConfig:
    if isinstance(identity, g.IdentityConfigExisting):
        return NativeIdentityConfig(
            ctypes.sizeof(NativeIdentityConfig),
            g.IdentityConfigKind.EXISTING,
            arena.bytes(identity.secret._view()),
            StringView(),
        )
    if isinstance(identity, g.IdentityConfigGenerateEphemeral):
        return NativeIdentityConfig(
            ctypes.sizeof(NativeIdentityConfig),
            g.IdentityConfigKind.GENERATE_EPHEMERAL,
            ByteView(),
            StringView(),
        )
    if isinstance(identity, g.IdentityConfigLoadOrCreate):
        return NativeIdentityConfig(
            ctypes.sizeof(NativeIdentityConfig),
            g.IdentityConfigKind.LOAD_OR_CREATE,
            ByteView(),
            arena.string(identity.path),
        )
    raise TypeError(f"unknown identity config {type(identity)!r}")


def _marshal_name(
    name: g.DestinationName,
    arena: _Arena,
) -> NativeDestinationName:
    if not name.app_name or not name.aspects or any(not value for value in name.aspects):
        raise ValueError("a destination requires a non-empty app name and aspects")
    aspects = [arena.string(value) for value in name.aspects]
    return NativeDestinationName(
        ctypes.sizeof(NativeDestinationName),
        arena.string(name.app_name),
        arena.array(StringView, aspects),
        len(aspects),
    )


def _marshal_destination(
    destination: g.DestinationConfig,
    arena: _Arena,
) -> NativeDestinationConfig:
    if isinstance(destination, g.DestinationConfigPlain):
        return NativeDestinationConfig(
            ctypes.sizeof(NativeDestinationConfig),
            g.DestinationConfigKind.PLAIN,
            _marshal_name(destination.name, arena),
            0,
            NativeIdentityConfig(),
            ByteView(),
        )
    if isinstance(destination, g.DestinationConfigSingle):
        identity = destination.identity
        if isinstance(identity, g.DestinationIdentityConfigHostIdentity):
            identity_kind = g.DestinationIdentityConfigKind.HOST_IDENTITY
            dedicated = NativeIdentityConfig()
        elif isinstance(identity, g.DestinationIdentityConfigDedicatedIdentity):
            identity_kind = g.DestinationIdentityConfigKind.DEDICATED_IDENTITY
            dedicated = _marshal_identity(identity.identity, arena)
        else:
            raise TypeError(f"unknown destination identity {type(identity)!r}")
        return NativeDestinationConfig(
            ctypes.sizeof(NativeDestinationConfig),
            g.DestinationConfigKind.SINGLE,
            _marshal_name(destination.name, arena),
            identity_kind,
            dedicated,
            arena.bytes(destination.announce_app_data or b""),
        )
    raise TypeError(f"unknown destination config {type(destination)!r}")


class Command:
    def __init__(self, native: NativeLibrary, handle: ctypes.c_void_p):
        self._native = native
        self._handle = handle
        self._closed = False
        self._lock = threading.RLock()
        self._pending: asyncio.Future[CommandSettlement] | None = None
        self._pending_done: threading.Event | None = None

    def __await__(self):
        return self.wait().__await__()

    async def wait(self) -> CommandSettlement:
        with self._lock:
            if self._closed:
                raise RuntimeError("command handle is closed")
            if self._pending is None:
                self._pending, self._pending_done = _daemon_future(
                    self._wait_blocking
                )
            pending = self._pending
        try:
            return await _await_daemon(pending)
        except asyncio.CancelledError:
            with self._lock:
                if not self._closed:
                    self._native.library.prns_command_interrupt_wait(self._handle)
            while self._pending_done is not None and not self._pending_done.is_set():
                await asyncio.sleep(0)
            while not pending.done():
                await asyncio.sleep(0)
            _consume_future(pending)
            raise
        finally:
            self.close()

    def _wait_blocking(self) -> CommandSettlement:
        result = CommandResult()
        result.struct_size = ctypes.sizeof(CommandResult)
        _check(
            self._native.library.prns_command_wait(
                self._handle,
                NEVER_TIMEOUT,
                ctypes.byref(result),
            )
        )
        detail = bytes_from_view(result.detail).decode()
        if result.failure:
            return CommandFailed(g.CommandFailureKind(result.failure), detail)
        outcome = g.CommandOutcomeKind(result.outcome)
        value = bytes_from_view(result.value)
        if outcome is g.CommandOutcomeKind.ANNOUNCED:
            decoded: g.CommandOutcome = g.CommandOutcomeAnnounced()
        elif outcome is g.CommandOutcomeKind.PACKET_DELIVERED:
            evidence = g.DeliveryEvidenceKind(result.evidence)
            decoded = g.CommandOutcomePacketDelivered(
                result.rtt_millis,
                evidence,
                None if evidence is g.DeliveryEvidenceKind.RESPONSE else g.PacketHash(value),
            )
        elif outcome is g.CommandOutcomeKind.LINK_CLOSE_QUEUED:
            decoded = g.CommandOutcomeLinkCloseQueued()
        elif outcome is g.CommandOutcomeKind.INTERFACE_ATTACHED:
            decoded = g.CommandOutcomeInterfaceAttached(g.InterfaceId(value))
        elif outcome is g.CommandOutcomeKind.INTERFACE_DETACHED:
            decoded = g.CommandOutcomeInterfaceDetached(g.InterfaceId(value))
        else:
            raise RuntimeError(f"unknown command outcome {outcome}")
        return CommandSucceeded(decoded)

    def close(self) -> None:
        with self._lock:
            if self._closed:
                return
            self._closed = True
            pending_done = self._pending_done
            if pending_done is not None and not pending_done.is_set():
                self._native.library.prns_command_interrupt_wait(self._handle)
        if pending_done is not None:
            pending_done.wait()
        self._native.library.prns_command_release(self._handle)


class ResourceStream:
    def __init__(self, native: NativeLibrary, handle: ctypes.c_void_p, total_bytes: int):
        self._native = native
        self._handle = handle
        self._closed = False
        self._lock = threading.RLock()
        self.total_bytes = total_bytes

    def __aiter__(self) -> ResourceStream:
        return self

    async def __anext__(self) -> bytes:
        with self._lock:
            if self._closed:
                raise StopAsyncIteration
            chunk = ByteView()
            finished = ctypes.c_uint8()
            _check(
                self._native.library.prns_resource_stream_next(
                    self._handle,
                    64 * 1024,
                    ctypes.byref(chunk),
                    ctypes.byref(finished),
                )
            )
            if finished.value:
                self.close()
                raise StopAsyncIteration
            value = bytes_from_view(chunk)
        await asyncio.sleep(0)
        return value

    async def __aenter__(self) -> ResourceStream:
        return self

    async def __aexit__(self, _type, _value, _traceback) -> None:
        self.close()

    def close(self) -> None:
        with self._lock:
            if not self._closed:
                self._native.library.prns_resource_stream_release(self._handle)
                self._closed = True


class EventStream(AsyncIterator[T]):
    def __init__(self, native: NativeLibrary, handle: ctypes.c_void_p):
        self._native = native
        self._handle = handle
        self._closed = False
        self._lock = threading.RLock()
        self._pending: asyncio.Future[T] | None = None
        self._pending_done: threading.Event | None = None
        self._released = threading.Event()

    def __aiter__(self) -> EventStream[T]:
        return self

    async def __anext__(self) -> T:
        with self._lock:
            if self._closed:
                raise StopAsyncIteration
            if self._pending is not None:
                raise RuntimeError("an event read is already pending")
            self._pending, self._pending_done = _daemon_future(self._next_blocking)
            pending = self._pending
        try:
            return await _await_daemon(pending)
        except StopAsyncIteration:
            self.close()
            raise
        except asyncio.CancelledError:
            with self._lock:
                if not self._closed:
                    self._native.library.prns_event_stream_interrupt_wait(self._handle)
            while self._pending_done is not None and not self._pending_done.is_set():
                await asyncio.sleep(0)
            while not pending.done():
                await asyncio.sleep(0)
            _consume_future(pending)
            raise
        finally:
            with self._lock:
                if self._pending is pending:
                    self._pending = None

    def _next_blocking(self) -> T:
        event = ctypes.c_void_p()
        status = _status(
            self._native.library.prns_event_stream_next(
                self._handle,
                NEVER_TIMEOUT,
                ctypes.byref(event),
            )
        )
        if status is g.Status.STOPPED:
            raise StopAsyncIteration
        if status is not g.Status.OK:
            raise PrnsError(status)
        try:
            return _decode_event(self._native, event)
        finally:
            self._native.library.prns_event_release(event)

    async def __aenter__(self) -> EventStream[T]:
        return self

    async def __aexit__(self, _type, _value, _traceback) -> None:
        await self.aclose()

    async def aclose(self) -> None:
        owns_release, pending_done = self._begin_close()
        if not owns_release:
            while not self._released.is_set():
                await asyncio.sleep(0)
            return
        while pending_done is not None and not pending_done.is_set():
            await asyncio.sleep(0)
        self._release()

    def close(self) -> None:
        owns_release, pending_done = self._begin_close()
        if not owns_release:
            self._released.wait()
            return
        if pending_done is not None:
            pending_done.wait()
        self._release()

    def _begin_close(self) -> tuple[bool, threading.Event | None]:
        with self._lock:
            if self._closed:
                return False, self._pending_done
            self._closed = True
            pending_done = self._pending_done
            if pending_done is not None and not pending_done.is_set():
                self._native.library.prns_event_stream_interrupt_wait(self._handle)
            return True, pending_done

    def _release(self) -> None:
        try:
            self._native.library.prns_event_stream_release(self._handle)
        finally:
            self._released.set()


def _event_bytes(native: NativeLibrary, event, field: g.EventField) -> bytes:
    value = ByteView()
    _check(native.library.prns_event_bytes(event, field, ctypes.byref(value)))
    return bytes_from_view(value)


def _optional_event_bytes(native: NativeLibrary, event, field: g.EventField) -> bytes | None:
    value = ByteView()
    status = _status(native.library.prns_event_bytes(event, field, ctypes.byref(value)))
    if status is g.Status.INVALID_ARGUMENT:
        return None
    if status is not g.Status.OK:
        raise PrnsError(status)
    return bytes_from_view(value)


def _event_string(native: NativeLibrary, event, field: g.EventField) -> str:
    value = StringView()
    _check(native.library.prns_event_string(event, field, ctypes.byref(value)))
    return bytes_from_view(value).decode()


def _event_u64(native: NativeLibrary, event, field: g.EventField) -> int:
    value = ctypes.c_uint64()
    _check(native.library.prns_event_u64(event, field, ctypes.byref(value)))
    return value.value


def _event_u8(native: NativeLibrary, event, field: g.EventField) -> int:
    value = _event_u64(native, event, field)
    if value > 255:
        raise RuntimeError(f"event field {field.name} exceeds u8")
    return value


def _event_u128(native: NativeLibrary, event, field: g.EventField) -> int:
    low = ctypes.c_uint64()
    high = ctypes.c_uint64()
    _check(
        native.library.prns_event_u128(
            event,
            field,
            ctypes.byref(low),
            ctypes.byref(high),
        )
    )
    return low.value | high.value << 64


def _decode_event(native: NativeLibrary, event):
    kind = native.library.prns_event_kind(event)
    try:
        application = g.ApplicationEventKind(kind)
    except ValueError:
        return _decode_diagnostic(native, event, g.DiagnosticEventKind(kind))
    f = g.EventField
    if application is g.ApplicationEventKind.SINGLE_DELIVERY:
        return g.ApplicationEventSingleDelivery(
            g.DestinationHash(_event_bytes(native, event, f.DESTINATION)),
            g.InterfaceId(_event_bytes(native, event, f.SOURCE_INTERFACE)),
            _event_bytes(native, event, f.PLAINTEXT),
        )
    if application is g.ApplicationEventKind.REQUEST:
        requester = _optional_event_bytes(native, event, f.REQUESTER)
        return g.ApplicationEventRequest(
            g.DestinationHash(_event_bytes(native, event, f.DESTINATION)),
            g.LinkId(_event_bytes(native, event, f.LINK_ID)),
            g.RequestId(_event_bytes(native, event, f.REQUEST_ID)),
            None if requester is None else g.IdentityHash(requester),
            g.RequestPathHash(_event_bytes(native, event, f.PATH_HASH)),
            _event_u64(native, event, f.RTT_MILLIS),
            _event_bytes(native, event, f.DATA),
        )
    if application is g.ApplicationEventKind.RESPONSE:
        return g.ApplicationEventResponse(
            g.LinkId(_event_bytes(native, event, f.LINK_ID)),
            g.RequestId(_event_bytes(native, event, f.REQUEST_ID)),
            _event_bytes(native, event, f.DATA),
        )
    if application is g.ApplicationEventKind.RESPONSE_SEGMENT:
        return g.ApplicationEventResponseSegment(
            g.LinkId(_event_bytes(native, event, f.LINK_ID)),
            g.RequestId(_event_bytes(native, event, f.REQUEST_ID)),
            _event_u64(native, event, f.SEGMENT_INDEX),
            _event_u64(native, event, f.TOTAL_SEGMENTS),
            _event_bytes(native, event, f.DATA),
        )
    if application is g.ApplicationEventKind.RESOURCE_AVAILABLE:
        resource = ctypes.c_void_p()
        _check(native.library.prns_event_resource_stream(event, ctypes.byref(resource)))
        metadata = _optional_event_bytes(native, event, f.METADATA)
        return g.ApplicationEventResourceAvailable(
            g.LinkId(_event_bytes(native, event, f.LINK_ID)),
            g.ResourceHash(_event_bytes(native, event, f.HASH)),
            metadata,
            ResourceStream(
                native,
                resource,
                _event_u64(native, event, f.TOTAL_BYTES),
            ),
        )
    if application is g.ApplicationEventKind.RESOURCE_SEGMENT:
        return g.ApplicationEventResourceSegment(
            g.LinkId(_event_bytes(native, event, f.LINK_ID)),
            g.ResourceHash(_event_bytes(native, event, f.ORIGINAL_HASH)),
            _event_u64(native, event, f.SEGMENT_INDEX),
            _event_u64(native, event, f.TOTAL_SEGMENTS),
            _optional_event_bytes(native, event, f.METADATA),
            _event_bytes(native, event, f.DATA),
        )
    if application is g.ApplicationEventKind.RESOURCE_NEEDS_DECOMPRESSION:
        return g.ApplicationEventResourceNeedsDecompression(
            g.LinkId(_event_bytes(native, event, f.LINK_ID)),
            g.ResourceHash(_event_bytes(native, event, f.HASH)),
            _event_bytes(native, event, f.STREAM),
            _event_u64(native, event, f.UNCOMPRESSED_DATA_BYTES),
        )
    if application is g.ApplicationEventKind.CHANNEL_MESSAGE:
        return g.ApplicationEventChannelMessage(
            g.LinkId(_event_bytes(native, event, f.LINK_ID)),
            _event_string(native, event, f.MESSAGE_TYPE),
            _event_bytes(native, event, f.DATA),
        )
    raise RuntimeError(f"unknown application event {application}")


def _decode_diagnostic(
    native: NativeLibrary,
    event,
    diagnostic: g.DiagnosticEventKind,
):
    f = g.EventField
    if diagnostic is g.DiagnosticEventKind.ANNOUNCE_HEARD:
        return g.DiagnosticEventAnnounceHeard(
            g.DestinationHash(_event_bytes(native, event, f.DESTINATION)),
            _event_u8(native, event, f.HOPS),
            g.InterfaceId(_event_bytes(native, event, f.SOURCE_INTERFACE)),
        )
    if diagnostic is g.DiagnosticEventKind.LINK_ESTABLISHED:
        return g.DiagnosticEventLinkEstablished(
            g.LinkId(_event_bytes(native, event, f.LINK_ID)),
            _event_u64(native, event, f.RTT_MILLIS),
        )
    if diagnostic is g.DiagnosticEventKind.PEER_IDENTIFIED:
        return g.DiagnosticEventPeerIdentified(
            g.LinkId(_event_bytes(native, event, f.LINK_ID)),
            g.IdentityHash(_event_bytes(native, event, f.IDENTITY)),
        )
    if diagnostic is g.DiagnosticEventKind.LINK_CLOSED:
        return g.DiagnosticEventLinkClosed(
            g.LinkId(_event_bytes(native, event, f.LINK_ID)),
            g.LinkClosedReason(_event_u64(native, event, f.REASON)),
        )
    if diagnostic is g.DiagnosticEventKind.LINK_INTERFACE_MISMATCH:
        return g.DiagnosticEventLinkInterfaceMismatch(
            g.LinkId(_event_bytes(native, event, f.LINK_ID)),
            g.InterfaceId(_event_bytes(native, event, f.ATTACHED_INTERFACE)),
            g.InterfaceId(_event_bytes(native, event, f.ARRIVED_ON)),
        )
    if diagnostic is g.DiagnosticEventKind.RESOURCE_ASSEMBLED:
        return g.DiagnosticEventResourceAssembled(
            g.LinkId(_event_bytes(native, event, f.LINK_ID)),
            g.ResourceHash(_event_bytes(native, event, f.ORIGINAL_HASH)),
            _event_u64(native, event, f.TOTAL_SIZE_BYTES),
        )
    if diagnostic is g.DiagnosticEventKind.RESOURCE_FAILED:
        return g.DiagnosticEventResourceFailed(
            g.LinkId(_event_bytes(native, event, f.LINK_ID)),
            g.ResourceHash(_event_bytes(native, event, f.HASH)),
            _event_string(native, event, f.CAUSE),
        )
    if diagnostic is g.DiagnosticEventKind.RESOURCE_SEND_PROGRESS:
        return g.DiagnosticEventResourceSendProgress(
            g.LinkId(_event_bytes(native, event, f.LINK_ID)),
            _event_u64(native, event, f.TRANSFERRED_BYTES),
            _event_u64(native, event, f.TOTAL_BYTES),
            _event_u64(native, event, f.PHYSICAL_TRANSFERRED_BYTES),
            _event_u64(native, event, f.SEGMENT_INDEX),
            _event_u64(native, event, f.TOTAL_SEGMENTS),
        )
    if diagnostic is g.DiagnosticEventKind.SELF_RATCHET_ROTATED:
        return g.DiagnosticEventSelfRatchetRotated(
            g.DestinationHash(_event_bytes(native, event, f.DESTINATION))
        )
    if diagnostic is g.DiagnosticEventKind.ANNOUNCE_HELD_DROPPED:
        return g.DiagnosticEventAnnounceHeldDropped(
            g.DestinationHash(_event_bytes(native, event, f.DESTINATION)),
            g.InterfaceId(_event_bytes(native, event, f.SOURCE_INTERFACE)),
            _event_string(native, event, f.CAUSE),
        )
    if diagnostic is g.DiagnosticEventKind.DELIVERED:
        return g.DiagnosticEventDelivered(_event_string(native, event, f.DETAIL))
    if diagnostic is g.DiagnosticEventKind.ROUTE_EXPIRED:
        return g.DiagnosticEventRouteExpired(
            g.DestinationHash(_event_bytes(native, event, f.DESTINATION))
        )
    if diagnostic is g.DiagnosticEventKind.ROUTE_EVICTED:
        return g.DiagnosticEventRouteEvicted(
            g.DestinationHash(_event_bytes(native, event, f.DESTINATION))
        )
    if diagnostic is g.DiagnosticEventKind.ROUTE_INTERFACE_GONE:
        return g.DiagnosticEventRouteInterfaceGone(
            g.DestinationHash(_event_bytes(native, event, f.DESTINATION))
        )
    if diagnostic is g.DiagnosticEventKind.ROUTE_DROPPED:
        return g.DiagnosticEventRouteDropped(
            g.DestinationHash(_event_bytes(native, event, f.DESTINATION))
        )
    if diagnostic is g.DiagnosticEventKind.BACKEND_DIAGNOSTIC:
        return g.DiagnosticEventBackendDiagnostic(
            _event_string(native, event, f.KIND),
            _event_string(native, event, f.DETAIL),
        )
    if diagnostic is g.DiagnosticEventKind.DIAGNOSTICS_DROPPED:
        return g.DiagnosticEventDiagnosticsDropped(
            _event_u128(native, event, f.DROPPED_COUNT)
        )
    raise RuntimeError(f"unknown diagnostic event {diagnostic}")


class Host:
    def __init__(self, native: NativeLibrary, handle: ctypes.c_void_p):
        self._native = native
        self._handle = handle
        self._closed = False
        self._lock = threading.RLock()

    @classmethod
    def create(cls, options: HostOptions) -> Host:
        native = NativeLibrary()
        info = ContractInfo()
        info.struct_size = ctypes.sizeof(ContractInfo)
        _check(native.library.prns_contract_info(ctypes.byref(info)))
        actual_version = bytes_from_view(info.product_version).decode()
        if (
            info.abi != g.HOST_CONTRACT_ABI
            or info.schema_version != g.SCHEMA_VERSION
            or actual_version != g.PRODUCT_VERSION
        ):
            raise ContractMismatchError(
                info.abi,
                info.schema_version,
                actual_version,
            )
        with _Arena() as arena:
            destinations = [
                _marshal_destination(destination, arena)
                for destination in options.destinations
            ]
            capabilities = [
                ctypes.c_uint32(value)
                for value in options.required_capabilities
            ]
            native_options = NativeHostOptions()
            native_options.struct_size = ctypes.sizeof(NativeHostOptions)
            native_options.required_abi = g.HOST_CONTRACT_ABI
            native_options.required_product_version = arena.string(
                g.PRODUCT_VERSION
            )
            native_options.limits = NativeLimits(
                ctypes.sizeof(NativeLimits),
                options.limits.pending_commands,
                options.limits.application_events,
                options.limits.retained_event_bytes,
                options.limits.diagnostics,
            )
            native_options.role = options.role
            native_options.identity = _marshal_identity(options.identity, arena)
            native_options.destinations = arena.array(
                NativeDestinationConfig,
                destinations,
            )
            native_options.destination_count = len(destinations)
            native_options.required_capabilities = arena.array(
                ctypes.c_uint32,
                capabilities,
            )
            native_options.required_capability_count = len(capabilities)
            handle = ctypes.c_void_p()
            _check(
                native.library.prns_host_create(
                    ctypes.byref(native_options),
                    ctypes.byref(handle),
                )
            )
        return cls(native, handle)

    @property
    @_host_operation
    def lifecycle(self) -> LifecycleSnapshot:
        value = Lifecycle()
        value.struct_size = ctypes.sizeof(Lifecycle)
        _check(
            self._native.library.prns_host_lifecycle(
                self._handle,
                ctypes.byref(value),
            )
        )
        phase = g.LifecyclePhase(value.phase)
        reason = g.StopReason(value.reason) if phase is g.LifecyclePhase.STOPPED else None
        return LifecycleSnapshot(value.revision, phase, reason)

    @property
    @_host_operation
    def identity_hash(self) -> g.IdentityHash:
        value = ByteView()
        _check(
            self._native.library.prns_host_identity_hash(
                self._handle,
                ctypes.byref(value),
            )
        )
        return g.IdentityHash(bytes_from_view(value))

    @property
    @_host_operation
    def destination_hashes(self) -> tuple[g.DestinationHash, ...]:
        count = self._native.library.prns_host_destination_count(self._handle)
        values = []
        for index in range(count):
            value = ByteView()
            _check(
                self._native.library.prns_host_destination_hash(
                    self._handle,
                    index,
                    ctypes.byref(value),
                )
            )
            values.append(g.DestinationHash(bytes_from_view(value)))
        return tuple(values)

    @_host_operation
    def submit(self, command: g.HostCommand) -> Command:
        with _Arena() as arena:
            handle = ctypes.c_void_p()
            if isinstance(command, g.HostCommandAnnounce):
                destination = arena.bytes(command.destination.value)
                if command.interface is None:
                    interface = None
                else:
                    interface_value = arena.bytes(command.interface.value)
                    interface = ctypes.byref(interface_value)
                status = self._native.library.prns_host_announce(
                    self._handle,
                    destination,
                    interface,
                    ctypes.byref(handle),
                )
            elif isinstance(command, g.HostCommandSendSinglePacket):
                status = self._native.library.prns_host_send_single_packet(
                    self._handle,
                    arena.bytes(command.destination.value),
                    arena.bytes(command.payload),
                    ctypes.byref(handle),
                )
            elif isinstance(command, g.HostCommandCloseLink):
                status = self._native.library.prns_host_close_link(
                    self._handle,
                    arena.bytes(command.link_id.value),
                    ctypes.byref(handle),
                )
            elif isinstance(command, g.HostCommandAttachTcpServer):
                kind, value = _marshal_bitrate(command.bitrate)
                status = self._native.library.prns_host_attach_tcp_server(
                    self._handle,
                    arena.string(command.bind),
                    kind,
                    value,
                    ctypes.byref(handle),
                )
            elif isinstance(command, g.HostCommandAttachTcpClient):
                kind, value = _marshal_bitrate(command.bitrate)
                status = self._native.library.prns_host_attach_tcp_client(
                    self._handle,
                    arena.string(command.target),
                    kind,
                    value,
                    ctypes.byref(handle),
                )
            elif isinstance(command, g.HostCommandAttachUdp):
                kind, value = _marshal_bitrate(command.bitrate)
                status = self._native.library.prns_host_attach_udp(
                    self._handle,
                    arena.string(command.local),
                    arena.string(command.peer),
                    kind,
                    value,
                    ctypes.byref(handle),
                )
            elif isinstance(command, g.HostCommandDetachInterface):
                status = self._native.library.prns_host_detach_interface(
                    self._handle,
                    arena.bytes(command.interface.value),
                    ctypes.byref(handle),
                )
            else:
                raise TypeError(f"unknown host command {type(command)!r}")
            _check(status)
        return Command(self._native, handle)

    async def announce(
        self,
        destination: g.DestinationHash,
        interface: g.InterfaceId | None = None,
    ) -> CommandSettlement:
        return await self.submit(g.HostCommandAnnounce(destination, interface))

    async def send_single_packet(
        self,
        destination: g.DestinationHash,
        payload: bytes,
    ) -> CommandSettlement:
        return await self.submit(g.HostCommandSendSinglePacket(destination, payload))

    async def close_link(self, link_id: g.LinkId) -> CommandSettlement:
        return await self.submit(g.HostCommandCloseLink(link_id))

    async def attach_tcp_server(
        self,
        bind: str,
        bitrate: g.Bitrate,
    ) -> CommandSettlement:
        return await self.submit(g.HostCommandAttachTcpServer(bind, bitrate))

    async def attach_tcp_client(
        self,
        target: str,
        bitrate: g.Bitrate,
    ) -> CommandSettlement:
        return await self.submit(g.HostCommandAttachTcpClient(target, bitrate))

    async def attach_udp(
        self,
        local: str,
        peer: str,
        bitrate: g.Bitrate,
    ) -> CommandSettlement:
        return await self.submit(g.HostCommandAttachUdp(local, peer, bitrate))

    async def detach_interface(
        self,
        interface: g.InterfaceId,
    ) -> CommandSettlement:
        return await self.submit(g.HostCommandDetachInterface(interface))

    def claim_events(self) -> StreamClaim[EventStream[g.ApplicationEvent]]:
        return self._claim(
            self._native.library.prns_host_claim_application_events,
            "application_events",
        )

    def claim_diagnostics(self) -> StreamClaim[EventStream[g.DiagnosticEvent]]:
        return self._claim(
            self._native.library.prns_host_claim_diagnostics,
            "diagnostics",
        )

    @_host_operation
    def _claim(self, function, lane: str):
        stream = ctypes.c_void_p()
        status = _status(function(self._handle, ctypes.byref(stream)))
        if status is g.Status.ALREADY_CLAIMED:
            return StreamAlreadyClaimed(lane)
        if status is not g.Status.OK:
            raise PrnsError(status)
        return StreamClaimed(EventStream(self._native, stream))

    async def __aenter__(self) -> Host:
        self._require_open()
        return self

    async def __aexit__(self, _type, _value, _traceback) -> None:
        await self.aclose()

    async def aclose(self) -> None:
        with self._lock:
            if not self._closed:
                _check(self._native.library.prns_host_stop(self._handle))
                self._native.library.prns_host_release(self._handle)
                self._closed = True

    def _require_open(self) -> None:
        if self._closed:
            raise RuntimeError("host is closed")


def _marshal_bitrate(bitrate: g.Bitrate) -> tuple[g.BitrateKind, int]:
    if isinstance(bitrate, g.BitrateAuto):
        return g.BitrateKind.AUTO, 0
    if isinstance(bitrate, g.BitrateBitsPerSecond):
        if bitrate.value < 5:
            raise ValueError("bitrate must be at least 5 bits per second")
        return g.BitrateKind.BITS_PER_SECOND, bitrate.value
    raise TypeError(f"unknown bitrate {type(bitrate)!r}")
