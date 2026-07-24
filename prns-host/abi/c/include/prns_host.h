#ifndef PRNS_HOST_H
#define PRNS_HOST_H

#include <stddef.h>
#include <stdint.h>

#if defined(_WIN32) && defined(PRNS_HOST_BUILD)
#define PRNS_HOST_API __declspec(dllexport)
#elif defined(_WIN32)
#define PRNS_HOST_API __declspec(dllimport)
#else
#define PRNS_HOST_API
#endif

#if defined(__cplusplus)
extern "C" {
#endif

#define PRNS_HOST_CONTRACT_ABI UINT32_C(1)
#define PRNS_HOST_SCHEMA_VERSION UINT32_C(1)
#define PRNS_DESTINATION_HASH_LENGTH UINT32_C(16)
#define PRNS_IDENTITY_HASH_LENGTH UINT32_C(16)
#define PRNS_INTERFACE_ID_LENGTH UINT32_C(8)
#define PRNS_LINK_ID_LENGTH UINT32_C(16)
#define PRNS_PACKET_HASH_LENGTH UINT32_C(32)
#define PRNS_REQUEST_ID_LENGTH UINT32_C(16)
#define PRNS_REQUEST_PATH_HASH_LENGTH UINT32_C(16)
#define PRNS_RESOURCE_HASH_LENGTH UINT32_C(32)
#define PRNS_IDENTITY_SECRET_LENGTH UINT32_C(64)
#define PRNS_BALANCED_PENDING_COMMANDS UINT64_C(256)
#define PRNS_BALANCED_APPLICATION_EVENTS UINT64_C(1024)
#define PRNS_BALANCED_RETAINED_EVENT_BYTES UINT64_C(8388608)
#define PRNS_BALANCED_DIAGNOSTICS UINT64_C(1024)

typedef uint32_t PrnsStatus;
#define PRNS_STATUS_OK UINT32_C(0)
#define PRNS_STATUS_INVALID_ARGUMENT UINT32_C(1)
#define PRNS_STATUS_CONTRACT_MISMATCH UINT32_C(2)
#define PRNS_STATUS_INVALID_HANDLE UINT32_C(3)
#define PRNS_STATUS_NOT_READY UINT32_C(4)
#define PRNS_STATUS_ALREADY_CLAIMED UINT32_C(5)
#define PRNS_STATUS_WOULD_BLOCK UINT32_C(6)
#define PRNS_STATUS_TIMED_OUT UINT32_C(7)
#define PRNS_STATUS_QUEUE_FULL UINT32_C(8)
#define PRNS_STATUS_STOPPED UINT32_C(9)
#define PRNS_STATUS_BACKEND_FAILED UINT32_C(10)
#define PRNS_STATUS_PANIC UINT32_C(11)
#define PRNS_STATUS_INTERRUPTED UINT32_C(12)

typedef uint32_t PrnsBackendKind;
#define PRNS_BACKEND_KIND_NATIVE UINT32_C(1)
#define PRNS_BACKEND_KIND_BROWSER UINT32_C(2)
#define PRNS_BACKEND_KIND_COOPERATIVE UINT32_C(3)

typedef uint32_t PrnsCapability;
#define PRNS_CAPABILITY_LOOPBACK UINT32_C(1)
#define PRNS_CAPABILITY_TCP_CLIENT UINT32_C(2)
#define PRNS_CAPABILITY_TCP_SERVER UINT32_C(3)
#define PRNS_CAPABILITY_UDP UINT32_C(4)
#define PRNS_CAPABILITY_SERIAL UINT32_C(5)
#define PRNS_CAPABILITY_USB UINT32_C(6)
#define PRNS_CAPABILITY_BLUETOOTH UINT32_C(7)
#define PRNS_CAPABILITY_WIFI UINT32_C(8)
#define PRNS_CAPABILITY_WEB_SOCKET UINT32_C(9)
#define PRNS_CAPABILITY_BROWSER_RENDEZVOUS UINT32_C(10)
#define PRNS_CAPABILITY_I2P UINT32_C(11)
#define PRNS_CAPABILITY_WEAVE UINT32_C(12)

typedef uint32_t PrnsHostRole;
#define PRNS_HOST_ROLE_ENDPOINT UINT32_C(1)
#define PRNS_HOST_ROLE_TRANSPORT UINT32_C(2)

typedef uint32_t PrnsIdentityConfigKind;
#define PRNS_IDENTITY_CONFIG_KIND_EXISTING UINT32_C(1)
#define PRNS_IDENTITY_CONFIG_KIND_GENERATE_EPHEMERAL UINT32_C(2)
#define PRNS_IDENTITY_CONFIG_KIND_LOAD_OR_CREATE UINT32_C(3)

typedef uint32_t PrnsDestinationConfigKind;
#define PRNS_DESTINATION_CONFIG_KIND_PLAIN UINT32_C(1)
#define PRNS_DESTINATION_CONFIG_KIND_SINGLE UINT32_C(2)

typedef uint32_t PrnsDestinationIdentityConfigKind;
#define PRNS_DESTINATION_IDENTITY_CONFIG_KIND_HOST_IDENTITY UINT32_C(1)
#define PRNS_DESTINATION_IDENTITY_CONFIG_KIND_DEDICATED_IDENTITY UINT32_C(2)

typedef uint32_t PrnsBitrateKind;
#define PRNS_BITRATE_KIND_AUTO UINT32_C(1)
#define PRNS_BITRATE_KIND_BITS_PER_SECOND UINT32_C(2)

typedef uint32_t PrnsResponseTimeoutKind;
#define PRNS_RESPONSE_TIMEOUT_KIND_LINK_DEFAULT UINT32_C(1)
#define PRNS_RESPONSE_TIMEOUT_KIND_EXACT UINT32_C(2)

typedef uint32_t PrnsResourceCompressionKind;
#define PRNS_RESOURCE_COMPRESSION_KIND_AUTO UINT32_C(1)
#define PRNS_RESOURCE_COMPRESSION_KIND_NEVER UINT32_C(2)

typedef uint32_t PrnsResourceStrategyKind;
#define PRNS_RESOURCE_STRATEGY_KIND_REFUSE UINT32_C(1)
#define PRNS_RESOURCE_STRATEGY_KIND_ACCEPT UINT32_C(2)

typedef uint32_t PrnsRequestPolicy;
#define PRNS_REQUEST_POLICY_ALLOW_NONE UINT32_C(1)
#define PRNS_REQUEST_POLICY_ALLOW_ALL UINT32_C(2)
#define PRNS_REQUEST_POLICY_ALLOW_LIST UINT32_C(3)

typedef uint32_t PrnsCommandOutcomeKind;
#define PRNS_COMMAND_OUTCOME_KIND_ANNOUNCED UINT32_C(1)
#define PRNS_COMMAND_OUTCOME_KIND_PACKET_DELIVERED UINT32_C(2)
#define PRNS_COMMAND_OUTCOME_KIND_LINK_CLOSE_QUEUED UINT32_C(3)
#define PRNS_COMMAND_OUTCOME_KIND_INTERFACE_ATTACHED UINT32_C(4)
#define PRNS_COMMAND_OUTCOME_KIND_INTERFACE_DETACHED UINT32_C(5)
#define PRNS_COMMAND_OUTCOME_KIND_LINK_ESTABLISHED UINT32_C(6)
#define PRNS_COMMAND_OUTCOME_KIND_PATH_DISCOVERED UINT32_C(7)
#define PRNS_COMMAND_OUTCOME_KIND_IDENTIFIED UINT32_C(8)
#define PRNS_COMMAND_OUTCOME_KIND_RESPONSE_RECEIVED UINT32_C(9)
#define PRNS_COMMAND_OUTCOME_KIND_RESPONSE_SENT UINT32_C(10)
#define PRNS_COMMAND_OUTCOME_KIND_RESOURCE_SENT UINT32_C(11)
#define PRNS_COMMAND_OUTCOME_KIND_RESOURCE_STRATEGY_SET UINT32_C(12)
#define PRNS_COMMAND_OUTCOME_KIND_REQUESTER_ALLOWED UINT32_C(13)

typedef uint32_t PrnsCommandFailureKind;
#define PRNS_COMMAND_FAILURE_KIND_NODE_STOPPED UINT32_C(1)
#define PRNS_COMMAND_FAILURE_KIND_BUSY UINT32_C(2)
#define PRNS_COMMAND_FAILURE_KIND_PAYLOAD_TOO_LARGE UINT32_C(3)
#define PRNS_COMMAND_FAILURE_KIND_UNKNOWN_DESTINATION UINT32_C(4)
#define PRNS_COMMAND_FAILURE_KIND_NOT_SINGLE_DESTINATION UINT32_C(5)
#define PRNS_COMMAND_FAILURE_KIND_ANNOUNCE_APP_DATA_TOO_LONG UINT32_C(6)
#define PRNS_COMMAND_FAILURE_KIND_UNKNOWN_INTERFACE UINT32_C(7)
#define PRNS_COMMAND_FAILURE_KIND_NO_ROUTE_TO_DESTINATION UINT32_C(8)
#define PRNS_COMMAND_FAILURE_KIND_NOT_DIRECTLY_REACHABLE UINT32_C(9)
#define PRNS_COMMAND_FAILURE_KIND_PACKET_CULLED UINT32_C(10)
#define PRNS_COMMAND_FAILURE_KIND_DELIVERY_TIMED_OUT UINT32_C(11)
#define PRNS_COMMAND_FAILURE_KIND_INVALID_BITRATE UINT32_C(12)
#define PRNS_COMMAND_FAILURE_KIND_BIND_FAILED UINT32_C(13)
#define PRNS_COMMAND_FAILURE_KIND_WRITE_FAILED UINT32_C(14)
#define PRNS_COMMAND_FAILURE_KIND_UNSUPPORTED_BY_BACKEND UINT32_C(15)
#define PRNS_COMMAND_FAILURE_KIND_UNKNOWN_LINK UINT32_C(16)
#define PRNS_COMMAND_FAILURE_KIND_LINK_NOT_ACTIVE UINT32_C(17)
#define PRNS_COMMAND_FAILURE_KIND_ENTROPY_UNAVAILABLE UINT32_C(18)
#define PRNS_COMMAND_FAILURE_KIND_NOT_LINK_INITIATOR UINT32_C(19)
#define PRNS_COMMAND_FAILURE_KIND_IDENTITY_NOT_HELD UINT32_C(20)
#define PRNS_COMMAND_FAILURE_KIND_UNKNOWN_REQUEST_HANDLER UINT32_C(21)
#define PRNS_COMMAND_FAILURE_KIND_REQUEST_POLICY_NOT_ALLOW_LIST UINT32_C(22)
#define PRNS_COMMAND_FAILURE_KIND_REQUEST_ALLOW_LIST_FULL UINT32_C(23)
#define PRNS_COMMAND_FAILURE_KIND_LINK_BUSY UINT32_C(24)
#define PRNS_COMMAND_FAILURE_KIND_RESOURCE_TABLE_FULL UINT32_C(25)
#define PRNS_COMMAND_FAILURE_KIND_RESOURCE_METADATA_TOO_LARGE UINT32_C(26)
#define PRNS_COMMAND_FAILURE_KIND_RESOURCE_REJECTED_BY_PEER UINT32_C(27)
#define PRNS_COMMAND_FAILURE_KIND_RESOURCE_SEQUENCING_FAILED UINT32_C(28)
#define PRNS_COMMAND_FAILURE_KIND_RESOURCE_PREDECESSOR_FAILED UINT32_C(29)
#define PRNS_COMMAND_FAILURE_KIND_CHANNEL_WINDOW_FULL UINT32_C(30)
#define PRNS_COMMAND_FAILURE_KIND_CHANNEL_UNTRACKABLE UINT32_C(31)
#define PRNS_COMMAND_FAILURE_KIND_INVALID_CHANNEL_MESSAGE_TYPE UINT32_C(32)

typedef uint32_t PrnsDeliveryEvidenceKind;
#define PRNS_DELIVERY_EVIDENCE_KIND_EXPLICIT_PROOF UINT32_C(1)
#define PRNS_DELIVERY_EVIDENCE_KIND_IMPLICIT_PROOF UINT32_C(2)
#define PRNS_DELIVERY_EVIDENCE_KIND_RESPONSE UINT32_C(3)

typedef uint32_t PrnsLifecyclePhase;
#define PRNS_LIFECYCLE_PHASE_STARTING UINT32_C(1)
#define PRNS_LIFECYCLE_PHASE_RUNNING UINT32_C(2)
#define PRNS_LIFECYCLE_PHASE_STOPPING UINT32_C(3)
#define PRNS_LIFECYCLE_PHASE_STOPPED UINT32_C(4)
#define PRNS_LIFECYCLE_PHASE_FAILED UINT32_C(5)

typedef uint32_t PrnsStopReason;
#define PRNS_STOP_REASON_REQUESTED UINT32_C(1)
#define PRNS_STOP_REASON_BACKEND_EXITED UINT32_C(2)

typedef uint32_t PrnsLinkClosedReason;
#define PRNS_LINK_CLOSED_REASON_TIMEOUT UINT32_C(1)
#define PRNS_LINK_CLOSED_REASON_PEER_CLOSED UINT32_C(2)
#define PRNS_LINK_CLOSED_REASON_MALFORMED_RTT UINT32_C(3)

typedef uint32_t PrnsApplicationEventKind;
#define PRNS_APPLICATION_EVENT_KIND_SINGLE_DELIVERY UINT32_C(100)
#define PRNS_APPLICATION_EVENT_KIND_REQUEST UINT32_C(101)
#define PRNS_APPLICATION_EVENT_KIND_RESPONSE UINT32_C(102)
#define PRNS_APPLICATION_EVENT_KIND_RESPONSE_SEGMENT UINT32_C(103)
#define PRNS_APPLICATION_EVENT_KIND_RESOURCE_AVAILABLE UINT32_C(104)
#define PRNS_APPLICATION_EVENT_KIND_RESOURCE_SEGMENT UINT32_C(105)
#define PRNS_APPLICATION_EVENT_KIND_RESOURCE_NEEDS_DECOMPRESSION UINT32_C(106)
#define PRNS_APPLICATION_EVENT_KIND_CHANNEL_MESSAGE UINT32_C(107)

typedef uint32_t PrnsDiagnosticEventKind;
#define PRNS_DIAGNOSTIC_EVENT_KIND_ANNOUNCE_HEARD UINT32_C(200)
#define PRNS_DIAGNOSTIC_EVENT_KIND_LINK_ESTABLISHED UINT32_C(201)
#define PRNS_DIAGNOSTIC_EVENT_KIND_PEER_IDENTIFIED UINT32_C(202)
#define PRNS_DIAGNOSTIC_EVENT_KIND_LINK_CLOSED UINT32_C(203)
#define PRNS_DIAGNOSTIC_EVENT_KIND_LINK_INTERFACE_MISMATCH UINT32_C(204)
#define PRNS_DIAGNOSTIC_EVENT_KIND_RESOURCE_ASSEMBLED UINT32_C(205)
#define PRNS_DIAGNOSTIC_EVENT_KIND_RESOURCE_FAILED UINT32_C(206)
#define PRNS_DIAGNOSTIC_EVENT_KIND_RESOURCE_SEND_PROGRESS UINT32_C(207)
#define PRNS_DIAGNOSTIC_EVENT_KIND_SELF_RATCHET_ROTATED UINT32_C(208)
#define PRNS_DIAGNOSTIC_EVENT_KIND_ANNOUNCE_HELD_DROPPED UINT32_C(209)
#define PRNS_DIAGNOSTIC_EVENT_KIND_DELIVERED UINT32_C(210)
#define PRNS_DIAGNOSTIC_EVENT_KIND_ROUTE_EXPIRED UINT32_C(211)
#define PRNS_DIAGNOSTIC_EVENT_KIND_ROUTE_EVICTED UINT32_C(212)
#define PRNS_DIAGNOSTIC_EVENT_KIND_ROUTE_INTERFACE_GONE UINT32_C(213)
#define PRNS_DIAGNOSTIC_EVENT_KIND_ROUTE_DROPPED UINT32_C(214)
#define PRNS_DIAGNOSTIC_EVENT_KIND_BACKEND_DIAGNOSTIC UINT32_C(215)
#define PRNS_DIAGNOSTIC_EVENT_KIND_DIAGNOSTICS_DROPPED UINT32_C(216)

typedef uint32_t PrnsEventField;
#define PRNS_EVENT_FIELD_DESTINATION UINT32_C(1)
#define PRNS_EVENT_FIELD_SOURCE_INTERFACE UINT32_C(2)
#define PRNS_EVENT_FIELD_PLAINTEXT UINT32_C(3)
#define PRNS_EVENT_FIELD_LINK_ID UINT32_C(4)
#define PRNS_EVENT_FIELD_REQUEST_ID UINT32_C(5)
#define PRNS_EVENT_FIELD_REQUESTER UINT32_C(6)
#define PRNS_EVENT_FIELD_PATH_HASH UINT32_C(7)
#define PRNS_EVENT_FIELD_RTT_MILLIS UINT32_C(8)
#define PRNS_EVENT_FIELD_DATA UINT32_C(9)
#define PRNS_EVENT_FIELD_SEGMENT_INDEX UINT32_C(10)
#define PRNS_EVENT_FIELD_TOTAL_SEGMENTS UINT32_C(11)
#define PRNS_EVENT_FIELD_HASH UINT32_C(12)
#define PRNS_EVENT_FIELD_ORIGINAL_HASH UINT32_C(13)
#define PRNS_EVENT_FIELD_METADATA UINT32_C(14)
#define PRNS_EVENT_FIELD_TOTAL_BYTES UINT32_C(15)
#define PRNS_EVENT_FIELD_STREAM_ID UINT32_C(16)
#define PRNS_EVENT_FIELD_UNCOMPRESSED_DATA_BYTES UINT32_C(17)
#define PRNS_EVENT_FIELD_MESSAGE_TYPE UINT32_C(18)
#define PRNS_EVENT_FIELD_IDENTITY UINT32_C(19)
#define PRNS_EVENT_FIELD_REASON UINT32_C(20)
#define PRNS_EVENT_FIELD_ATTACHED_INTERFACE UINT32_C(21)
#define PRNS_EVENT_FIELD_ARRIVED_ON UINT32_C(22)
#define PRNS_EVENT_FIELD_TOTAL_SIZE_BYTES UINT32_C(23)
#define PRNS_EVENT_FIELD_CAUSE UINT32_C(24)
#define PRNS_EVENT_FIELD_TRANSFERRED_BYTES UINT32_C(25)
#define PRNS_EVENT_FIELD_PHYSICAL_TRANSFERRED_BYTES UINT32_C(26)
#define PRNS_EVENT_FIELD_DETAIL UINT32_C(27)
#define PRNS_EVENT_FIELD_KIND UINT32_C(28)
#define PRNS_EVENT_FIELD_DROPPED_COUNT UINT32_C(29)
#define PRNS_EVENT_FIELD_HOPS UINT32_C(30)
#define PRNS_EVENT_FIELD_STREAM UINT32_C(31)

/*
 * Ownership and lifetime contract:
 * - Input byte/string views and configuration arrays are borrowed only for
 *   the duration of the call; prns_host_create copies all retained data.
 * - Every non-null opaque handle returned through an out parameter has one
 *   owner and must be passed exactly once to its matching *_release function.
 * - Release and interrupt functions accept NULL and do nothing. Functions
 *   with status results reject other required NULL arguments.
 * - A release must not race another operation on the same handle. Interrupt
 *   may race its matching wait; release only after that wait has returned.
 * - UINT32_MAX is the infinite timeout for command and event-stream waits.
 * - All exported calls contain Rust panics and report PRNS_STATUS_PANIC where
 *   the function has a status result; no Rust unwinding crosses this ABI.
 */

typedef struct PrnsHost PrnsHost;
typedef struct PrnsCommand PrnsCommand;
typedef struct PrnsEventStream PrnsEventStream;
typedef struct PrnsEvent PrnsEvent;
typedef struct PrnsResourceStream PrnsResourceStream;

typedef struct PrnsByteView {
    const uint8_t *data;
    size_t length;
} PrnsByteView;

typedef struct PrnsStringView {
    const uint8_t *data;
    size_t length;
} PrnsStringView;

typedef struct PrnsContractInfo {
    size_t struct_size;
    uint32_t abi;
    uint32_t schema_version;
    PrnsStringView product_version;
} PrnsContractInfo;

typedef struct PrnsLimits {
    size_t struct_size;
    size_t pending_commands;
    size_t application_events;
    size_t retained_event_bytes;
    size_t diagnostics;
} PrnsLimits;

typedef struct PrnsIdentityConfig {
    size_t struct_size;
    PrnsIdentityConfigKind kind;
    PrnsByteView secret;
    PrnsStringView path;
} PrnsIdentityConfig;

typedef struct PrnsDestinationName {
    size_t struct_size;
    PrnsStringView app_name;
    const PrnsStringView *aspects;
    size_t aspect_count;
} PrnsDestinationName;

typedef struct PrnsRequestHandlerConfig {
    size_t struct_size;
    PrnsStringView path;
    PrnsRequestPolicy policy;
} PrnsRequestHandlerConfig;

typedef struct PrnsDestinationConfig {
    size_t struct_size;
    PrnsDestinationConfigKind kind;
    PrnsDestinationName name;
    PrnsDestinationIdentityConfigKind identity_kind;
    PrnsIdentityConfig dedicated_identity;
    PrnsByteView announce_app_data;
    const PrnsRequestHandlerConfig *request_handlers;
    size_t request_handler_count;
} PrnsDestinationConfig;

typedef struct PrnsHostOptions {
    size_t struct_size;
    uint32_t required_abi;
    PrnsStringView required_product_version;
    PrnsLimits limits;
    PrnsHostRole role;
    PrnsIdentityConfig identity;
    const PrnsDestinationConfig *destinations;
    size_t destination_count;
    const PrnsCapability *required_capabilities;
    size_t required_capability_count;
} PrnsHostOptions;

typedef struct PrnsLifecycle {
    size_t struct_size;
    uint64_t revision;
    PrnsLifecyclePhase phase;
    uint32_t reason;
} PrnsLifecycle;

typedef struct PrnsCommandResult {
    size_t struct_size;
    PrnsCommandOutcomeKind outcome;
    PrnsCommandFailureKind failure;
    PrnsDeliveryEvidenceKind evidence;
    uint64_t rtt_millis;
    PrnsByteView value;
    PrnsStringView detail;
} PrnsCommandResult;

/* product_version points to process-lifetime static storage. */
PRNS_HOST_API PrnsStatus prns_contract_info(PrnsContractInfo *out_info);
PRNS_HOST_API PrnsStatus prns_host_create(const PrnsHostOptions *options, PrnsHost **out_host);
PRNS_HOST_API void prns_host_release(PrnsHost *host);
PRNS_HOST_API PrnsStatus prns_host_lifecycle(const PrnsHost *host, PrnsLifecycle *out_lifecycle);
/* Returned host views remain valid until prns_host_release. */
PRNS_HOST_API PrnsStatus prns_host_identity_hash(const PrnsHost *host, PrnsByteView *out_hash);
PRNS_HOST_API size_t prns_host_destination_count(const PrnsHost *host);
PRNS_HOST_API PrnsStatus prns_host_destination_hash(const PrnsHost *host, size_t index, PrnsByteView *out_hash);
PRNS_HOST_API PrnsStatus prns_host_announce(PrnsHost *host, PrnsByteView destination, const PrnsByteView *interface_id, PrnsCommand **out_command);
PRNS_HOST_API PrnsStatus prns_host_send_single_packet(PrnsHost *host, PrnsByteView destination, PrnsByteView payload, PrnsCommand **out_command);
PRNS_HOST_API PrnsStatus prns_host_close_link(PrnsHost *host, PrnsByteView link_id, PrnsCommand **out_command);
PRNS_HOST_API PrnsStatus prns_host_attach_tcp_server(PrnsHost *host, PrnsStringView bind, PrnsBitrateKind bitrate_kind, uint64_t bitrate_bps, PrnsCommand **out_command);
PRNS_HOST_API PrnsStatus prns_host_attach_tcp_client(PrnsHost *host, PrnsStringView target, PrnsBitrateKind bitrate_kind, uint64_t bitrate_bps, PrnsCommand **out_command);
PRNS_HOST_API PrnsStatus prns_host_attach_udp(PrnsHost *host, PrnsStringView local, PrnsStringView peer, PrnsBitrateKind bitrate_kind, uint64_t bitrate_bps, PrnsCommand **out_command);
PRNS_HOST_API PrnsStatus prns_host_detach_interface(PrnsHost *host, PrnsByteView interface_id, PrnsCommand **out_command);
PRNS_HOST_API PrnsStatus prns_host_establish_link(PrnsHost *host, PrnsByteView destination, PrnsCommand **out_command);
PRNS_HOST_API PrnsStatus prns_host_request_path(PrnsHost *host, PrnsByteView destination, PrnsCommand **out_command);
PRNS_HOST_API PrnsStatus prns_host_identify(PrnsHost *host, PrnsByteView link_id, PrnsByteView identity, PrnsCommand **out_command);
PRNS_HOST_API PrnsStatus prns_host_send_link_packet(PrnsHost *host, PrnsByteView link_id, PrnsByteView payload, PrnsCommand **out_command);
PRNS_HOST_API PrnsStatus prns_host_request(PrnsHost *host, PrnsByteView link_id, PrnsByteView path_hash, PrnsByteView payload, PrnsResponseTimeoutKind timeout_kind, uint64_t timeout_millis, PrnsCommand **out_command);
PRNS_HOST_API PrnsStatus prns_host_respond(PrnsHost *host, PrnsByteView link_id, PrnsByteView request_id, uint64_t request_rtt_millis, PrnsByteView payload, PrnsCommand **out_command);
PRNS_HOST_API PrnsStatus prns_host_send_resource(PrnsHost *host, PrnsByteView link_id, PrnsByteView payload, const PrnsByteView *packed_metadata, PrnsResourceCompressionKind compression_kind, PrnsCommand **out_command);
PRNS_HOST_API PrnsStatus prns_host_set_link_resource_strategy(PrnsHost *host, PrnsByteView link_id, PrnsResourceStrategyKind strategy_kind, uint64_t maximum_uncompressed_bytes, uint8_t accept_compressed, PrnsCommand **out_command);
PRNS_HOST_API PrnsStatus prns_host_set_destination_resource_strategy(PrnsHost *host, PrnsByteView destination, PrnsResourceStrategyKind strategy_kind, uint64_t maximum_uncompressed_bytes, uint8_t accept_compressed, PrnsCommand **out_command);
PRNS_HOST_API PrnsStatus prns_host_send_channel_message(PrnsHost *host, PrnsByteView link_id, uint16_t message_type, PrnsByteView payload, PrnsCommand **out_command);
PRNS_HOST_API PrnsStatus prns_host_allow_requester(PrnsHost *host, PrnsByteView destination, PrnsByteView path_hash, PrnsByteView identity, PrnsCommand **out_command);
PRNS_HOST_API PrnsStatus prns_host_stop(PrnsHost *host);
/* Result views remain valid until prns_command_release. */
PRNS_HOST_API PrnsStatus prns_command_wait(PrnsCommand *command, uint32_t timeout_millis, PrnsCommandResult *out_result);
PRNS_HOST_API void prns_command_interrupt_wait(PrnsCommand *command);
PRNS_HOST_API void prns_command_release(PrnsCommand *command);
PRNS_HOST_API PrnsStatus prns_host_claim_application_events(PrnsHost *host, PrnsEventStream **out_stream);
PRNS_HOST_API PrnsStatus prns_host_claim_diagnostics(PrnsHost *host, PrnsEventStream **out_stream);
PRNS_HOST_API void prns_event_stream_interrupt_wait(PrnsEventStream *stream);
PRNS_HOST_API void prns_event_stream_release(PrnsEventStream *stream);
PRNS_HOST_API PrnsStatus prns_event_stream_next(PrnsEventStream *stream, uint32_t timeout_millis, PrnsEvent **out_event);
PRNS_HOST_API void prns_event_release(PrnsEvent *event);
PRNS_HOST_API uint32_t prns_event_kind(const PrnsEvent *event);
/* Event views remain valid until prns_event_release. */
PRNS_HOST_API PrnsStatus prns_event_bytes(const PrnsEvent *event, PrnsEventField field, PrnsByteView *out_value);
PRNS_HOST_API PrnsStatus prns_event_string(const PrnsEvent *event, PrnsEventField field, PrnsStringView *out_value);
PRNS_HOST_API PrnsStatus prns_event_u64(const PrnsEvent *event, PrnsEventField field, uint64_t *out_value);
PRNS_HOST_API PrnsStatus prns_event_u128(const PrnsEvent *event, PrnsEventField field, uint64_t *out_low, uint64_t *out_high);
/* A resource may be claimed once and remains owned after its event is released. */
PRNS_HOST_API PrnsStatus prns_event_resource_stream(PrnsEvent *event, PrnsResourceStream **out_stream);
PRNS_HOST_API void prns_resource_stream_release(PrnsResourceStream *stream);
/* out_chunk remains valid until the next call or release on this stream. */
PRNS_HOST_API PrnsStatus prns_resource_stream_next(PrnsResourceStream *stream, size_t maximum_bytes, PrnsByteView *out_chunk, uint8_t *out_finished);

#if defined(__cplusplus)
}
#endif

#endif
