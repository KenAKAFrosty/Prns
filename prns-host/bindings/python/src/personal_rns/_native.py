from __future__ import annotations

import ctypes
import ctypes.util
import os
import sys
from pathlib import Path

ReadinessCallback = ctypes.CFUNCTYPE(None, ctypes.c_void_p)


class ByteView(ctypes.Structure):
    _fields_ = [("data", ctypes.POINTER(ctypes.c_uint8)), ("length", ctypes.c_size_t)]


class StringView(ctypes.Structure):
    _fields_ = [("data", ctypes.POINTER(ctypes.c_uint8)), ("length", ctypes.c_size_t)]


class ContractInfo(ctypes.Structure):
    _fields_ = [
        ("struct_size", ctypes.c_size_t),
        ("abi", ctypes.c_uint32),
        ("schema_version", ctypes.c_uint32),
        ("product_version", StringView),
    ]


class Limits(ctypes.Structure):
    _fields_ = [
        ("struct_size", ctypes.c_size_t),
        ("pending_commands", ctypes.c_size_t),
        ("application_events", ctypes.c_size_t),
        ("retained_event_bytes", ctypes.c_size_t),
        ("diagnostics", ctypes.c_size_t),
    ]


class IdentityConfig(ctypes.Structure):
    _fields_ = [
        ("struct_size", ctypes.c_size_t),
        ("kind", ctypes.c_uint32),
        ("secret", ByteView),
        ("path", StringView),
    ]


class DestinationName(ctypes.Structure):
    _fields_ = [
        ("struct_size", ctypes.c_size_t),
        ("app_name", StringView),
        ("aspects", ctypes.POINTER(StringView)),
        ("aspect_count", ctypes.c_size_t),
    ]


class RequestHandlerConfig(ctypes.Structure):
    _fields_ = [
        ("struct_size", ctypes.c_size_t),
        ("path", StringView),
        ("policy", ctypes.c_uint32),
    ]


class DestinationConfig(ctypes.Structure):
    _fields_ = [
        ("struct_size", ctypes.c_size_t),
        ("kind", ctypes.c_uint32),
        ("name", DestinationName),
        ("identity_kind", ctypes.c_uint32),
        ("dedicated_identity", IdentityConfig),
        ("announce_app_data", ByteView),
        ("request_handlers", ctypes.POINTER(RequestHandlerConfig)),
        ("request_handler_count", ctypes.c_size_t),
    ]


class HostOptions(ctypes.Structure):
    _fields_ = [
        ("struct_size", ctypes.c_size_t),
        ("required_abi", ctypes.c_uint32),
        ("required_product_version", StringView),
        ("limits", Limits),
        ("role", ctypes.c_uint32),
        ("identity", IdentityConfig),
        ("destinations", ctypes.POINTER(DestinationConfig)),
        ("destination_count", ctypes.c_size_t),
        ("required_capabilities", ctypes.POINTER(ctypes.c_uint32)),
        ("required_capability_count", ctypes.c_size_t),
    ]


class Lifecycle(ctypes.Structure):
    _fields_ = [
        ("struct_size", ctypes.c_size_t),
        ("revision", ctypes.c_uint64),
        ("phase", ctypes.c_uint32),
        ("reason", ctypes.c_uint32),
    ]


class CommandResult(ctypes.Structure):
    _fields_ = [
        ("struct_size", ctypes.c_size_t),
        ("outcome", ctypes.c_uint32),
        ("failure", ctypes.c_uint32),
        ("evidence", ctypes.c_uint32),
        ("rtt_millis", ctypes.c_uint64),
        ("value", ByteView),
        ("detail", StringView),
    ]


def bytes_from_view(view: ByteView | StringView) -> bytes:
    if not view.length:
        return b""
    return ctypes.string_at(view.data, view.length)


def library_path() -> str:
    configured = os.environ.get("PRNS_HOST_LIBRARY")
    if configured:
        return configured
    native = Path(__file__).with_name("native")
    names = {
        "win32": ("prns_host.dll",),
        "darwin": ("libprns_host.dylib",),
    }.get(sys.platform, ("libprns_host.so",))
    for name in names:
        candidate = native / name
        if candidate.is_file():
            return str(candidate)
    found = ctypes.util.find_library("prns_host")
    if found:
        return found
    raise RuntimeError(
        "Personal RNS native library is unavailable; install a platform wheel or set PRNS_HOST_LIBRARY"
    )


class NativeLibrary:
    def __init__(self):
        self.library = ctypes.CDLL(library_path())
        lib = self.library
        lib.prns_contract_info.argtypes = [ctypes.POINTER(ContractInfo)]
        lib.prns_contract_info.restype = ctypes.c_uint32
        lib.prns_host_create.argtypes = [
            ctypes.POINTER(HostOptions),
            ctypes.POINTER(ctypes.c_void_p),
        ]
        lib.prns_host_create.restype = ctypes.c_uint32
        lib.prns_host_release.argtypes = [ctypes.c_void_p]
        lib.prns_host_release.restype = None
        lib.prns_host_lifecycle.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(Lifecycle),
        ]
        lib.prns_host_lifecycle.restype = ctypes.c_uint32
        lib.prns_host_identity_hash.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ByteView),
        ]
        lib.prns_host_identity_hash.restype = ctypes.c_uint32
        lib.prns_host_destination_count.argtypes = [ctypes.c_void_p]
        lib.prns_host_destination_count.restype = ctypes.c_size_t
        lib.prns_host_destination_hash.argtypes = [
            ctypes.c_void_p,
            ctypes.c_size_t,
            ctypes.POINTER(ByteView),
        ]
        lib.prns_host_destination_hash.restype = ctypes.c_uint32
        lib.prns_host_announce.argtypes = [
            ctypes.c_void_p,
            ByteView,
            ctypes.POINTER(ByteView),
            ctypes.POINTER(ctypes.c_void_p),
        ]
        lib.prns_host_announce.restype = ctypes.c_uint32
        lib.prns_host_send_single_packet.argtypes = [
            ctypes.c_void_p,
            ByteView,
            ByteView,
            ctypes.POINTER(ctypes.c_void_p),
        ]
        lib.prns_host_send_single_packet.restype = ctypes.c_uint32
        lib.prns_host_close_link.argtypes = [
            ctypes.c_void_p,
            ByteView,
            ctypes.POINTER(ctypes.c_void_p),
        ]
        lib.prns_host_close_link.restype = ctypes.c_uint32
        for name in ("prns_host_attach_tcp_server", "prns_host_attach_tcp_client"):
            function = getattr(lib, name)
            function.argtypes = [
                ctypes.c_void_p,
                StringView,
                ctypes.c_uint32,
                ctypes.c_uint64,
                ctypes.POINTER(ctypes.c_void_p),
            ]
            function.restype = ctypes.c_uint32
        lib.prns_host_attach_udp.argtypes = [
            ctypes.c_void_p,
            StringView,
            StringView,
            ctypes.c_uint32,
            ctypes.c_uint64,
            ctypes.POINTER(ctypes.c_void_p),
        ]
        lib.prns_host_attach_udp.restype = ctypes.c_uint32
        lib.prns_host_detach_interface.argtypes = [
            ctypes.c_void_p,
            ByteView,
            ctypes.POINTER(ctypes.c_void_p),
        ]
        lib.prns_host_detach_interface.restype = ctypes.c_uint32
        for name in ("prns_host_establish_link", "prns_host_request_path"):
            function = getattr(lib, name)
            function.argtypes = [
                ctypes.c_void_p,
                ByteView,
                ctypes.POINTER(ctypes.c_void_p),
            ]
            function.restype = ctypes.c_uint32
        for name in ("prns_host_identify", "prns_host_send_link_packet"):
            function = getattr(lib, name)
            function.argtypes = [
                ctypes.c_void_p,
                ByteView,
                ByteView,
                ctypes.POINTER(ctypes.c_void_p),
            ]
            function.restype = ctypes.c_uint32
        lib.prns_host_request.argtypes = [
            ctypes.c_void_p,
            ByteView,
            ByteView,
            ByteView,
            ctypes.c_uint32,
            ctypes.c_uint64,
            ctypes.POINTER(ctypes.c_void_p),
        ]
        lib.prns_host_request.restype = ctypes.c_uint32
        lib.prns_host_respond.argtypes = [
            ctypes.c_void_p,
            ByteView,
            ByteView,
            ctypes.c_uint64,
            ByteView,
            ctypes.POINTER(ctypes.c_void_p),
        ]
        lib.prns_host_respond.restype = ctypes.c_uint32
        lib.prns_host_send_resource.argtypes = [
            ctypes.c_void_p,
            ByteView,
            ByteView,
            ctypes.POINTER(ByteView),
            ctypes.c_uint32,
            ctypes.POINTER(ctypes.c_void_p),
        ]
        lib.prns_host_send_resource.restype = ctypes.c_uint32
        for name in (
            "prns_host_set_link_resource_strategy",
            "prns_host_set_destination_resource_strategy",
        ):
            function = getattr(lib, name)
            function.argtypes = [
                ctypes.c_void_p,
                ByteView,
                ctypes.c_uint32,
                ctypes.c_uint64,
                ctypes.c_uint8,
                ctypes.POINTER(ctypes.c_void_p),
            ]
            function.restype = ctypes.c_uint32
        lib.prns_host_send_channel_message.argtypes = [
            ctypes.c_void_p,
            ByteView,
            ctypes.c_uint16,
            ByteView,
            ctypes.POINTER(ctypes.c_void_p),
        ]
        lib.prns_host_send_channel_message.restype = ctypes.c_uint32
        lib.prns_host_allow_requester.argtypes = [
            ctypes.c_void_p,
            ByteView,
            ByteView,
            ByteView,
            ctypes.POINTER(ctypes.c_void_p),
        ]
        lib.prns_host_allow_requester.restype = ctypes.c_uint32
        lib.prns_host_stop.argtypes = [ctypes.c_void_p]
        lib.prns_host_stop.restype = ctypes.c_uint32
        lib.prns_command_wait.argtypes = [
            ctypes.c_void_p,
            ctypes.c_uint32,
            ctypes.POINTER(CommandResult),
        ]
        lib.prns_command_wait.restype = ctypes.c_uint32
        lib.prns_command_register_readiness.argtypes = [
            ctypes.c_void_p,
            ReadinessCallback,
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_void_p),
        ]
        lib.prns_command_register_readiness.restype = ctypes.c_uint32
        lib.prns_command_interrupt_wait.argtypes = [ctypes.c_void_p]
        lib.prns_command_interrupt_wait.restype = None
        lib.prns_command_release.argtypes = [ctypes.c_void_p]
        lib.prns_command_release.restype = None
        lib.prns_host_claim_application_events.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_void_p),
        ]
        lib.prns_host_claim_application_events.restype = ctypes.c_uint32
        lib.prns_host_claim_diagnostics.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_void_p),
        ]
        lib.prns_host_claim_diagnostics.restype = ctypes.c_uint32
        lib.prns_event_stream_register_readiness.argtypes = [
            ctypes.c_void_p,
            ReadinessCallback,
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_void_p),
        ]
        lib.prns_event_stream_register_readiness.restype = ctypes.c_uint32
        lib.prns_readiness_registration_release.argtypes = [ctypes.c_void_p]
        lib.prns_readiness_registration_release.restype = None
        lib.prns_event_stream_interrupt_wait.argtypes = [ctypes.c_void_p]
        lib.prns_event_stream_interrupt_wait.restype = None
        lib.prns_event_stream_release.argtypes = [ctypes.c_void_p]
        lib.prns_event_stream_release.restype = None
        lib.prns_event_stream_next.argtypes = [
            ctypes.c_void_p,
            ctypes.c_uint32,
            ctypes.POINTER(ctypes.c_void_p),
        ]
        lib.prns_event_stream_next.restype = ctypes.c_uint32
        lib.prns_event_release.argtypes = [ctypes.c_void_p]
        lib.prns_event_release.restype = None
        lib.prns_event_kind.argtypes = [ctypes.c_void_p]
        lib.prns_event_kind.restype = ctypes.c_uint32
        lib.prns_event_bytes.argtypes = [
            ctypes.c_void_p,
            ctypes.c_uint32,
            ctypes.POINTER(ByteView),
        ]
        lib.prns_event_bytes.restype = ctypes.c_uint32
        lib.prns_event_string.argtypes = [
            ctypes.c_void_p,
            ctypes.c_uint32,
            ctypes.POINTER(StringView),
        ]
        lib.prns_event_string.restype = ctypes.c_uint32
        lib.prns_event_u64.argtypes = [
            ctypes.c_void_p,
            ctypes.c_uint32,
            ctypes.POINTER(ctypes.c_uint64),
        ]
        lib.prns_event_u64.restype = ctypes.c_uint32
        lib.prns_event_u128.argtypes = [
            ctypes.c_void_p,
            ctypes.c_uint32,
            ctypes.POINTER(ctypes.c_uint64),
            ctypes.POINTER(ctypes.c_uint64),
        ]
        lib.prns_event_u128.restype = ctypes.c_uint32
        lib.prns_event_resource_stream.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_void_p),
        ]
        lib.prns_event_resource_stream.restype = ctypes.c_uint32
        lib.prns_resource_stream_release.argtypes = [ctypes.c_void_p]
        lib.prns_resource_stream_release.restype = None
        lib.prns_resource_stream_next.argtypes = [
            ctypes.c_void_p,
            ctypes.c_size_t,
            ctypes.POINTER(ByteView),
            ctypes.POINTER(ctypes.c_uint8),
        ]
        lib.prns_resource_stream_next.restype = ctypes.c_uint32
