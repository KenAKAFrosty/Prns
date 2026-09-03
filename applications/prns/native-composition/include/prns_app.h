#ifndef PRNS_APP_H
#define PRNS_APP_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/** Maximum byte length accepted for a UTF-8 application storage path. */
#define PRNS_APP_MAX_PATH_BYTES 4096U

/** Maximum byte length accepted for a generated contract JSON input. */
#define PRNS_APP_MAX_INPUT_BYTES 65536U

/** Maximum UTF-8 byte length accepted for a CoreBluetooth restoration identifier. */
#define PRNS_APP_MAX_RESTORATION_IDENTIFIER_BYTES 1024U

/**
 * Owned, length-delimited UTF-8 JSON returned by the application ABI.
 *
 * The buffer is not NUL-terminated. Copy or consume exactly `len` bytes, then
 * pass the unchanged value to prns_app_bytes_free exactly once. A normal
 * domain result has the generated Rust contract shape. Only invalid ABI input
 * or a contained Rust panic returns the private bridge envelope
 * `{ "type": "bridgeFailure", "kind": "invalidInput" | "panic", ... }`.
 */
typedef struct PrnsAppBytes {
  uint8_t *ptr;
  size_t len;
} PrnsAppBytes;

/**
 * Return the immutable contract fingerprint as a borrowed NUL-terminated
 * string. The returned pointer remains valid for the process lifetime and must
 * not be freed.
 */
const char *prns_app_contract_fingerprint(void);

/** Return the immutable canonical Host-contract fingerprint. */
const char *prns_app_host_contract_fingerprint(void);

/** Inspect the primary identity stored below the private development root. */
PrnsAppBytes prns_app_inspect_identity(const uint8_t *path_ptr,
                                       size_t path_len);

/** Preview a raw identity credential without persisting it. */
PrnsAppBytes prns_app_preview_identity_import(const uint8_t *input_ptr,
                                              size_t input_len);

/** Generate and persist the primary identity. */
PrnsAppBytes prns_app_create_generated_identity(const uint8_t *path_ptr,
                                                size_t path_len);

/** Validate and persist one raw identity credential. */
PrnsAppBytes prns_app_create_imported_identity(
    const uint8_t *path_ptr, size_t path_len, const uint8_t *input_ptr,
    size_t input_len);

/**
 * Start the development node. `path_ptr` is the private storage path and
 * `input_ptr` is DevelopmentNodeStartInput JSON. Neither is NUL-terminated.
 */
PrnsAppBytes prns_app_start(const uint8_t *path_ptr, size_t path_len,
                            const uint8_t *input_ptr, size_t input_len);

/**
 * Synchronously create the application-owned CoreBluetooth restoration
 * managers and delegates without waiting for full node readiness. A matching
 * restoring start atomically consumes the retained prepared owner.
 */
PrnsAppBytes prns_app_prepare_apple_bluetooth_restoration(
    const uint8_t *path_ptr, size_t path_len,
    const uint8_t *central_identifier_ptr, size_t central_identifier_len,
    const uint8_t *peripheral_identifier_ptr,
    size_t peripheral_identifier_len);

/**
 * Start the development node with stable, application-owned CoreBluetooth
 * restoration identifiers. All buffers are length-delimited UTF-8; the
 * central and peripheral identifiers must be nonempty and distinct.
 */
PrnsAppBytes prns_app_start_with_apple_restoration(
    const uint8_t *path_ptr, size_t path_len, const uint8_t *input_ptr,
    size_t input_len, const uint8_t *central_identifier_ptr,
    size_t central_identifier_len, const uint8_t *peripheral_identifier_ptr,
    size_t peripheral_identifier_len);

/** Return the current authoritative development-node snapshot. */
PrnsAppBytes prns_app_snapshot(void);

/** List compatible LXMF peers retained by the current generation. */
PrnsAppBytes prns_app_list_lxmf_peers(void);

/** Measure UTF-8 title/content JSON against the direct Link-packet bound. */
PrnsAppBytes prns_app_measure_lxmf_text(const uint8_t *input_ptr,
                                        size_t input_len);

/** Emit the registered current-form LXMF announce. */
PrnsAppBytes prns_app_announce_lxmf(void);

/** Commit one direct LXMF text message to the durable queue. */
PrnsAppBytes prns_app_send_direct_text(const uint8_t *input_ptr,
                                       size_t input_len);

/** List one bounded page of durable LXMF messages, including while stopped. */
PrnsAppBytes prns_app_list_lxmf_messages(
    const uint8_t *path_ptr, size_t path_len, const uint8_t *input_ptr,
    size_t input_len);

/** Requeue a failed durable LXMF record using its exact stored wire. */
PrnsAppBytes prns_app_retry_lxmf_message(
    const uint8_t *path_ptr, size_t path_len, const uint8_t *input_ptr,
    size_t input_len);

/** Cancel a queued durable LXMF record using a Rust-owned timestamp. */
PrnsAppBytes prns_app_cancel_lxmf_message(
    const uint8_t *path_ptr, size_t path_len, const uint8_t *input_ptr,
    size_t input_len);

/**
 * Initiate pairing. `input_ptr` must address `input_len` immutable UTF-8 JSON
 * bytes matching InitiateRemoteControlPairingInput for the duration of the
 * call.
 */
PrnsAppBytes prns_app_initiate_pairing(const uint8_t *input_ptr,
                                       size_t input_len);

/**
 * Approve pairing. The input bytes must match
 * RemoteControlPairingDecisionInput and remain readable for the call.
 */
PrnsAppBytes prns_app_approve_pairing(const uint8_t *input_ptr,
                                      size_t input_len);

/**
 * Reject pairing. The input bytes must match
 * RemoteControlPairingDecisionInput and remain readable for the call.
 */
PrnsAppBytes prns_app_reject_pairing(const uint8_t *input_ptr,
                                     size_t input_len);

/**
 * Describe a paired target. The input bytes must match
 * DescribeRemoteControlTargetInput and remain readable for the call.
 */
PrnsAppBytes prns_app_describe_target(const uint8_t *input_ptr,
                                      size_t input_len);

/** Save a destination after Rust confirms its live authenticated identity. */
PrnsAppBytes prns_app_save_observed_destination(
    const uint8_t *path_ptr, size_t path_len, const uint8_t *input_ptr,
    size_t input_len);

/** Create one manually entered contact. */
PrnsAppBytes prns_app_create_manual_contact(
    const uint8_t *path_ptr, size_t path_len, const uint8_t *input_ptr,
    size_t input_len);

/** Set or clear one saved contact alias. */
PrnsAppBytes prns_app_set_contact_alias(
    const uint8_t *path_ptr, size_t path_len, const uint8_t *input_ptr,
    size_t input_len);

/** Set one saved contact's pin state. */
PrnsAppBytes prns_app_set_contact_pinned(
    const uint8_t *path_ptr, size_t path_len, const uint8_t *input_ptr,
    size_t input_len);

/** Delete one saved contact. */
PrnsAppBytes prns_app_delete_contact(const uint8_t *path_ptr, size_t path_len,
                                     const uint8_t *input_ptr,
                                     size_t input_len);

/** Read one saved contact. */
PrnsAppBytes prns_app_get_contact(const uint8_t *path_ptr, size_t path_len,
                                  const uint8_t *input_ptr, size_t input_len);

/** List all saved contacts in raw destination-byte order. */
PrnsAppBytes prns_app_list_contacts(const uint8_t *path_ptr, size_t path_len);

/** Stop the development node and wait for its generated stop outcome. */
PrnsAppBytes prns_app_stop(void);

/**
 * Stop the development node and remove its private storage. `path_ptr` must
 * address `path_len` immutable UTF-8 bytes for the duration of the call.
 */
PrnsAppBytes prns_app_reset(const uint8_t *path_ptr, size_t path_len);

/**
 * Release one unchanged result returned by this ABI. Passing `{ NULL, 0 }` is
 * a no-op. Any other pointer/length pair not returned by this exact library,
 * or releasing the same result more than once, is invalid.
 */
void prns_app_bytes_free(PrnsAppBytes bytes);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* PRNS_APP_H */
