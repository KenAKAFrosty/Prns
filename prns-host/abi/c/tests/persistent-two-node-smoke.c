#define _POSIX_C_SOURCE 200809L

#include "../include/prns_host.h"

#include <arpa/inet.h>
#include <errno.h>
#include <netinet/in.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

#ifdef __cplusplus
#define PRNS_ZERO {}
#else
#define PRNS_ZERO {0}
#endif

typedef struct JourneyFixture {
    uint32_t schema_version;
    char app_name[64];
    char aspects[4][64];
    size_t aspect_count;
    char announce_app_data_hex[256];
    char request_path[128];
    char path_hash_hex[64];
    char request_payload_hex[256];
    char response_hex[256];
    uint64_t timeout_millis;
    char chunks_hex[8][256];
    size_t chunk_count;
    char metadata_hex[256];
    uint64_t maximum_uncompressed_bytes;
    uint8_t accept_compressed;
} JourneyFixture;

static PrnsByteView byte_view(const uint8_t *data, size_t length) {
    PrnsByteView view = PRNS_ZERO;
    view.data = data;
    view.length = length;
    return view;
}

static PrnsStringView string_view(const char *value) {
    PrnsStringView view = PRNS_ZERO;
    view.data = (const uint8_t *)value;
    view.length = strlen(value);
    return view;
}

static char *read_text(const char *path) {
    FILE *file = fopen(path, "rb");
    long length;
    char *text;
    if (file == NULL) {
        return NULL;
    }
    if (fseek(file, 0, SEEK_END) != 0) {
        fclose(file);
        return NULL;
    }
    length = ftell(file);
    if (length < 0 || fseek(file, 0, SEEK_SET) != 0) {
        fclose(file);
        return NULL;
    }
    text = (char *)malloc((size_t)length + 1);
    if (text == NULL) {
        fclose(file);
        return NULL;
    }
    if (fread(text, 1, (size_t)length, file) != (size_t)length) {
        free(text);
        fclose(file);
        return NULL;
    }
    text[length] = '\0';
    fclose(file);
    return text;
}

static const char *json_value(const char *text, const char *key) {
    char needle[128];
    const char *match;
    if (snprintf(needle, sizeof(needle), "\"%s\"", key) < 0) {
        return NULL;
    }
    match = strstr(text, needle);
    if (match == NULL) {
        return NULL;
    }
    return strchr(match + strlen(needle), ':');
}

static int json_string(
    const char *text,
    const char *key,
    char *output,
    size_t capacity
) {
    const char *value = json_value(text, key);
    const char *start;
    const char *end;
    size_t length;
    if (value == NULL || capacity == 0) {
        return 0;
    }
    start = strchr(value, '"');
    if (start == NULL) {
        return 0;
    }
    start += 1;
    end = strchr(start, '"');
    if (end == NULL) {
        return 0;
    }
    length = (size_t)(end - start);
    if (length >= capacity) {
        return 0;
    }
    memcpy(output, start, length);
    output[length] = '\0';
    return 1;
}

static int json_u64(const char *text, const char *key, uint64_t *output) {
    const char *value = json_value(text, key);
    char *end;
    unsigned long long parsed;
    if (value == NULL) {
        return 0;
    }
    errno = 0;
    parsed = strtoull(value + 1, &end, 10);
    if (errno != 0 || end == value + 1) {
        return 0;
    }
    *output = (uint64_t)parsed;
    return 1;
}

static int json_bool(const char *text, const char *key, uint8_t *output) {
    const char *value = json_value(text, key);
    if (value == NULL) {
        return 0;
    }
    value += 1;
    while (*value == ' ' || *value == '\t' || *value == '\r' || *value == '\n') {
        value += 1;
    }
    if (strncmp(value, "true", 4) == 0) {
        *output = 1;
        return 1;
    }
    if (strncmp(value, "false", 5) == 0) {
        *output = 0;
        return 1;
    }
    return 0;
}

static int json_strings(
    const char *text,
    const char *key,
    char values[][256],
    size_t capacity,
    size_t *count
) {
    const char *value = json_value(text, key);
    const char *cursor;
    const char *end;
    size_t found = 0;
    if (value == NULL) {
        return 0;
    }
    cursor = strchr(value, '[');
    if (cursor == NULL) {
        return 0;
    }
    end = strchr(cursor, ']');
    if (end == NULL) {
        return 0;
    }
    while (cursor < end) {
        const char *start = strchr(cursor, '"');
        const char *finish;
        size_t length;
        if (start == NULL || start >= end) {
            break;
        }
        start += 1;
        finish = strchr(start, '"');
        if (finish == NULL || finish > end || found >= capacity) {
            return 0;
        }
        length = (size_t)(finish - start);
        if (length >= 256) {
            return 0;
        }
        memcpy(values[found], start, length);
        values[found][length] = '\0';
        found += 1;
        cursor = finish + 1;
    }
    *count = found;
    return found > 0;
}

static int load_fixture(const char *path, JourneyFixture *fixture) {
    char *text = read_text(path);
    uint64_t schema_version = 0;
    char aspect_values[4][256] = PRNS_ZERO;
    size_t index;
    if (text == NULL) {
        return 0;
    }
    memset(fixture, 0, sizeof(*fixture));
    if (!json_u64(text, "schemaVersion", &schema_version) ||
        schema_version > UINT32_MAX ||
        !json_string(text, "appName", fixture->app_name, sizeof(fixture->app_name)) ||
        !json_strings(text, "aspects", aspect_values, 4, &fixture->aspect_count) ||
        !json_string(text, "announceAppDataHex", fixture->announce_app_data_hex, sizeof(fixture->announce_app_data_hex)) ||
        !json_string(text, "path", fixture->request_path, sizeof(fixture->request_path)) ||
        !json_string(text, "pathHashHex", fixture->path_hash_hex, sizeof(fixture->path_hash_hex)) ||
        !json_string(text, "payloadHex", fixture->request_payload_hex, sizeof(fixture->request_payload_hex)) ||
        !json_string(text, "responseHex", fixture->response_hex, sizeof(fixture->response_hex)) ||
        !json_u64(text, "timeoutMillis", &fixture->timeout_millis) ||
        !json_strings(text, "chunksHex", fixture->chunks_hex, 8, &fixture->chunk_count) ||
        !json_string(text, "metadataHex", fixture->metadata_hex, sizeof(fixture->metadata_hex)) ||
        !json_u64(text, "maximumUncompressedBytes", &fixture->maximum_uncompressed_bytes) ||
        !json_bool(text, "acceptCompressed", &fixture->accept_compressed)) {
        free(text);
        return 0;
    }
    fixture->schema_version = (uint32_t)schema_version;
    for (index = 0; index < fixture->aspect_count; index += 1) {
        if (strlen(aspect_values[index]) >= sizeof(fixture->aspects[index])) {
            free(text);
            return 0;
        }
        strcpy(fixture->aspects[index], aspect_values[index]);
    }
    free(text);
    return 1;
}

static int validate_interface_fixture(const char *path) {
    static const char *names[19] = {
        "AutoLan", "TcpClient", "TcpServer", "Udp", "Serial", "Kiss",
        "Ax25Kiss", "RNode", "MultiRNode", "Pipe", "BackboneClient",
        "BackboneServer", "I2p", "Weave", "AutomaticUsb",
        "AutomaticBluetoothLe", "WebSocketClient", "WebSocketServer",
        "BrowserRendezvous"
    };
    char *text = read_text(path);
    const char *cursor;
    size_t count = 0;
    PrnsInterfaceConfig configs[19] = PRNS_ZERO;
    size_t index;
    if (text == NULL) {
        return 0;
    }
    cursor = text;
    while ((cursor = strstr(cursor, "\"kind\"")) != NULL) {
        const char *colon = strchr(cursor, ':');
        const char *start = colon == NULL ? NULL : strchr(colon, '"');
        const char *end = start == NULL ? NULL : strchr(start + 1, '"');
        size_t length;
        if (start == NULL || end == NULL) {
            free(text);
            return 0;
        }
        length = (size_t)(end - start - 1);
        if (!((length == 4 && strncmp(start + 1, "Auto", 4) == 0) ||
              (length == 13 && strncmp(start + 1, "BitsPerSecond", 13) == 0))) {
            if (count >= 19 || strlen(names[count]) != length ||
                strncmp(start + 1, names[count], length) != 0) {
                free(text);
                return 0;
            }
            count += 1;
        }
        cursor = end + 1;
    }
    free(text);
    if (count != 19) {
        return 0;
    }
    for (index = 0; index < count; index += 1) {
        configs[index].struct_size = sizeof(configs[index]);
        configs[index].kind = (PrnsInterfaceKind)(index + 1);
        if (configs[index].struct_size != sizeof(PrnsInterfaceConfig) ||
            configs[index].kind != (PrnsInterfaceKind)(index + 1)) {
            return 0;
        }
    }
    return 1;
}

static int hex_bytes(
    const char *hex,
    uint8_t *output,
    size_t capacity,
    size_t *length
) {
    size_t count = strlen(hex) / 2;
    size_t index;
    if (strlen(hex) % 2 != 0 || count > capacity) {
        return 0;
    }
    for (index = 0; index < count; index += 1) {
        unsigned int value;
        if (sscanf(hex + index * 2, "%2x", &value) != 1) {
            return 0;
        }
        output[index] = (uint8_t)value;
    }
    *length = count;
    return 1;
}

static int reserve_loopback_port(uint16_t *port) {
    int descriptor = socket(AF_INET, SOCK_STREAM, 0);
    struct sockaddr_in address = PRNS_ZERO;
    socklen_t length = sizeof(address);
    if (descriptor < 0) {
        return 0;
    }
    address.sin_family = AF_INET;
    address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    address.sin_port = 0;
    if (bind(descriptor, (const struct sockaddr *)&address, sizeof(address)) != 0 ||
        getsockname(descriptor, (struct sockaddr *)&address, &length) != 0) {
        close(descriptor);
        return 0;
    }
    *port = ntohs(address.sin_port);
    close(descriptor);
    return 1;
}

static void delay_millis(long millis) {
    struct timespec duration = PRNS_ZERO;
    duration.tv_sec = millis / 1000;
    duration.tv_nsec = (millis % 1000) * 1000000L;
    nanosleep(&duration, NULL);
}

static int wait_outcome(
    PrnsIssuedCommand **command,
    PrnsCommandOutcomeKind expected,
    uint8_t *value,
    size_t capacity,
    size_t *value_length
) {
    PrnsCommandResult result = PRNS_ZERO;
    PrnsStatus status;
    result.struct_size = sizeof(result);
    status = prns_command_wait(*command, UINT32_C(10000), &result);
    if (status == PRNS_STATUS_OK && result.failure == 0 && result.outcome == expected) {
        if (value_length != NULL) {
            if (result.value.length > capacity) {
                status = PRNS_STATUS_INVALID_ARGUMENT;
            } else {
                memcpy(value, result.value.data, result.value.length);
                *value_length = result.value.length;
            }
        }
    } else {
        status = PRNS_STATUS_BACKEND_FAILED;
    }
    prns_command_release(*command);
    *command = NULL;
    return status == PRNS_STATUS_OK;
}

static int create_host(
    const char *root,
    uint8_t server,
    const JourneyFixture *fixture,
    const uint8_t *announce_data,
    size_t announce_data_length,
    const char *product_version,
    PrnsHost **host
) {
    char identity_path[1024];
    char state_path[1024];
    PrnsStringView aspects[4] = PRNS_ZERO;
    PrnsRequestHandlerConfig handler = PRNS_ZERO;
    PrnsDestinationConfig destination = PRNS_ZERO;
    PrnsCapability capability;
    PrnsHostOptions options = PRNS_ZERO;
    size_t index;
    if (snprintf(identity_path, sizeof(identity_path), "%s/identity", root) < 0 ||
        snprintf(state_path, sizeof(state_path), "%s/state", root) < 0) {
        return 0;
    }
    for (index = 0; index < fixture->aspect_count; index += 1) {
        aspects[index] = string_view(fixture->aspects[index]);
    }
    handler.struct_size = sizeof(handler);
    handler.path = string_view(fixture->request_path);
    handler.policy = PRNS_REQUEST_POLICY_ALLOW_ALL;
    destination.struct_size = sizeof(destination);
    destination.kind = PRNS_DESTINATION_CONFIG_KIND_SINGLE;
    destination.name.struct_size = sizeof(destination.name);
    destination.name.app_name = string_view(fixture->app_name);
    destination.name.aspects = aspects;
    destination.name.aspect_count = fixture->aspect_count;
    destination.identity_kind = PRNS_DESTINATION_IDENTITY_CONFIG_KIND_HOST_IDENTITY;
    destination.announce_app_data = byte_view(announce_data, announce_data_length);
    destination.request_handlers = &handler;
    destination.request_handler_count = 1;
    capability = server ? PRNS_CAPABILITY_TCP_SERVER : PRNS_CAPABILITY_TCP_CLIENT;
    options.struct_size = sizeof(options);
    options.required_abi = PRNS_HOST_CONTRACT_ABI;
    options.required_schema_version = PRNS_HOST_SCHEMA_VERSION;
    options.required_product_version = string_view(product_version);
    options.limits.struct_size = sizeof(options.limits);
    options.limits.pending_commands = (size_t)PRNS_BALANCED_PENDING_COMMANDS;
    options.limits.application_events = (size_t)PRNS_BALANCED_APPLICATION_EVENTS;
    options.limits.retained_event_bytes = (size_t)PRNS_BALANCED_RETAINED_EVENT_BYTES;
    options.limits.diagnostics = (size_t)PRNS_BALANCED_DIAGNOSTICS;
    options.role = PRNS_HOST_ROLE_ENDPOINT;
    options.identity.struct_size = sizeof(options.identity);
    options.identity.kind = PRNS_IDENTITY_CONFIG_KIND_LOAD_OR_CREATE;
    options.identity.path = string_view(identity_path);
    options.destinations = server ? &destination : NULL;
    options.destination_count = server ? 1 : 0;
    options.required_capabilities = &capability;
    options.required_capability_count = 1;
    options.persistence.struct_size = sizeof(options.persistence);
    options.persistence.kind = PRNS_PERSISTENCE_CONFIG_KIND_DIRECTORY;
    options.persistence.path = string_view(state_path);
    return prns_host_create(&options, host) == PRNS_STATUS_OK;
}

static int snapshot_has_route(
    PrnsHost *host,
    const uint8_t *destination,
    uint8_t *restored
) {
    PrnsHostInspection *inspection = NULL;
    PrnsHostSnapshot snapshot = PRNS_ZERO;
    size_t index;
    int found = 0;
    if (prns_host_snapshot(host, UINT32_C(5000), &inspection) != PRNS_STATUS_OK) {
        return 0;
    }
    snapshot.struct_size = sizeof(snapshot);
    if (prns_host_snapshot_read(inspection, &snapshot) != PRNS_STATUS_OK) {
        prns_host_snapshot_release(inspection);
        return 0;
    }
    if (restored != NULL) {
        *restored = snapshot.persistence.restored;
    }
    for (index = 0; index < snapshot.route_count; index += 1) {
        if (snapshot.routes[index].destination.length == PRNS_DESTINATION_HASH_LENGTH &&
            memcmp(snapshot.routes[index].destination.data, destination, PRNS_DESTINATION_HASH_LENGTH) == 0) {
            found = 1;
        }
    }
    prns_host_snapshot_release(inspection);
    return found;
}

static int next_application_event(
    PrnsEventStream *stream,
    uint32_t expected,
    PrnsEvent **event
) {
    while (1) {
        if (prns_event_stream_next(stream, UINT32_C(5000), event) != PRNS_STATUS_OK) {
            return 0;
        }
        if (prns_event_kind(*event) == expected) {
            return 1;
        }
        prns_event_release(*event);
        *event = NULL;
    }
}

int main(int argc, char **argv) {
    JourneyFixture fixture = PRNS_ZERO;
    PrnsContractInfo contract = PRNS_ZERO;
    PrnsHost *server = NULL;
    PrnsHost *client = NULL;
    PrnsHost *restored_server = NULL;
    PrnsHost *restored_client = NULL;
    PrnsEventStream *events = NULL;
    PrnsEvent *event = NULL;
    PrnsResourceStream *resource_stream = NULL;
    PrnsResourceUpload *upload = NULL;
    PrnsIssuedCommand *command = NULL;
    PrnsIssuedCommand *request_command = NULL;
    PrnsInterfaceConfig interface_config = PRNS_ZERO;
    PrnsByteView view = PRNS_ZERO;
    uint8_t announce_data[128] = PRNS_ZERO;
    uint8_t request_payload[128] = PRNS_ZERO;
    uint8_t response_payload[128] = PRNS_ZERO;
    uint8_t path_hash[PRNS_REQUEST_PATH_HASH_LENGTH] = PRNS_ZERO;
    uint8_t metadata[128] = PRNS_ZERO;
    uint8_t chunks[8][128] = PRNS_ZERO;
    size_t chunk_lengths[8] = PRNS_ZERO;
    uint8_t resource_payload[1024] = PRNS_ZERO;
    uint8_t received_resource[1024] = PRNS_ZERO;
    uint8_t server_identity[PRNS_IDENTITY_HASH_LENGTH] = PRNS_ZERO;
    uint8_t client_identity[PRNS_IDENTITY_HASH_LENGTH] = PRNS_ZERO;
    uint8_t destination_hash[PRNS_DESTINATION_HASH_LENGTH] = PRNS_ZERO;
    uint8_t link_id[PRNS_LINK_ID_LENGTH] = PRNS_ZERO;
    uint8_t request_link_id[PRNS_LINK_ID_LENGTH] = PRNS_ZERO;
    uint8_t request_id[PRNS_REQUEST_ID_LENGTH] = PRNS_ZERO;
    uint8_t command_value[256] = PRNS_ZERO;
    size_t announce_data_length = 0;
    size_t request_payload_length = 0;
    size_t response_payload_length = 0;
    size_t path_hash_length = 0;
    size_t metadata_length = 0;
    size_t resource_payload_length = 0;
    size_t received_resource_length = 0;
    size_t command_value_length = 0;
    uint64_t request_rtt_millis = 0;
    uint16_t port = 0;
    char server_address[64];
    char server_root[1024];
    char client_root[1024];
    uint8_t restored = 0;
    uint8_t finished = 0;
    int routed = 0;
    int result = 1;
    size_t index;

#define REQUIRE(condition, message) \
    do { \
        if (!(condition)) { \
            fprintf(stderr, "C journey failed: %s\n", message); \
            goto cleanup; \
        } \
    } while (0)

    REQUIRE(argc == 5, "expected journey fixture, interface fixture, persistence root, and product version arguments");
    REQUIRE(load_fixture(argv[1], &fixture), "could not load fixture");
    REQUIRE(validate_interface_fixture(argv[2]), "interface fixture does not match the C contract");
    REQUIRE(fixture.schema_version == PRNS_HOST_SCHEMA_VERSION, "schema mismatch");
    contract.struct_size = sizeof(contract);
    REQUIRE(prns_contract_info(&contract) == PRNS_STATUS_OK, "contract info failed");
    REQUIRE(contract.abi == PRNS_HOST_CONTRACT_ABI, "ABI mismatch");
    REQUIRE(contract.schema_version == PRNS_HOST_SCHEMA_VERSION, "native schema mismatch");
    REQUIRE(hex_bytes(fixture.announce_app_data_hex, announce_data, sizeof(announce_data), &announce_data_length), "invalid announce data");
    REQUIRE(hex_bytes(fixture.request_payload_hex, request_payload, sizeof(request_payload), &request_payload_length), "invalid request payload");
    REQUIRE(hex_bytes(fixture.response_hex, response_payload, sizeof(response_payload), &response_payload_length), "invalid response payload");
    REQUIRE(hex_bytes(fixture.path_hash_hex, path_hash, sizeof(path_hash), &path_hash_length), "invalid path hash");
    REQUIRE(path_hash_length == PRNS_REQUEST_PATH_HASH_LENGTH, "path hash length mismatch");
    REQUIRE(hex_bytes(fixture.metadata_hex, metadata, sizeof(metadata), &metadata_length), "invalid metadata");
    for (index = 0; index < fixture.chunk_count; index += 1) {
        REQUIRE(hex_bytes(fixture.chunks_hex[index], chunks[index], sizeof(chunks[index]), &chunk_lengths[index]), "invalid resource chunk");
        REQUIRE(resource_payload_length + chunk_lengths[index] <= sizeof(resource_payload), "resource fixture too large");
        memcpy(resource_payload + resource_payload_length, chunks[index], chunk_lengths[index]);
        resource_payload_length += chunk_lengths[index];
    }
    REQUIRE(reserve_loopback_port(&port), "could not reserve loopback port");
    REQUIRE(snprintf(server_address, sizeof(server_address), "127.0.0.1:%u", (unsigned int)port) > 0, "could not format address");
    REQUIRE(snprintf(server_root, sizeof(server_root), "%s/server", argv[3]) > 0, "could not format server root");
    REQUIRE(snprintf(client_root, sizeof(client_root), "%s/client", argv[3]) > 0, "could not format client root");
    REQUIRE(create_host(server_root, 1, &fixture, announce_data, announce_data_length, argv[4], &server), "server creation failed");
    REQUIRE(create_host(client_root, 0, &fixture, announce_data, announce_data_length, argv[4], &client), "client creation failed");
    REQUIRE(prns_host_identity_hash(server, &view) == PRNS_STATUS_OK && view.length == sizeof(server_identity), "server identity unavailable");
    memcpy(server_identity, view.data, sizeof(server_identity));
    REQUIRE(prns_host_identity_hash(client, &view) == PRNS_STATUS_OK && view.length == sizeof(client_identity), "client identity unavailable");
    memcpy(client_identity, view.data, sizeof(client_identity));
    REQUIRE(prns_host_destination_hash(server, 0, &view) == PRNS_STATUS_OK && view.length == sizeof(destination_hash), "destination unavailable");
    memcpy(destination_hash, view.data, sizeof(destination_hash));
    REQUIRE(prns_host_claim_application_events(server, &events) == PRNS_STATUS_OK, "event claim failed");

    interface_config.struct_size = sizeof(interface_config);
    interface_config.kind = PRNS_INTERFACE_KIND_TCP_SERVER;
    interface_config.bind = string_view(server_address);
    interface_config.bitrate_kind = PRNS_BITRATE_KIND_AUTO;
    REQUIRE(prns_host_attach_interface(server, &interface_config, &command) == PRNS_STATUS_OK, "server attach submission failed");
    REQUIRE(wait_outcome(&command, PRNS_COMMAND_OUTCOME_KIND_INTERFACE_ATTACHED, command_value, sizeof(command_value), &command_value_length), "server attach failed");
    memset(&interface_config, 0, sizeof(interface_config));
    interface_config.struct_size = sizeof(interface_config);
    interface_config.kind = PRNS_INTERFACE_KIND_TCP_CLIENT;
    interface_config.target = string_view(server_address);
    interface_config.bitrate_kind = PRNS_BITRATE_KIND_AUTO;
    REQUIRE(prns_host_attach_interface(client, &interface_config, &command) == PRNS_STATUS_OK, "client attach submission failed");
    REQUIRE(wait_outcome(&command, PRNS_COMMAND_OUTCOME_KIND_INTERFACE_ATTACHED, command_value, sizeof(command_value), &command_value_length), "client attach failed");

    for (index = 0; index < 50 && !routed; index += 1) {
        routed = snapshot_has_route(client, destination_hash, NULL);
        if (!routed) {
            REQUIRE(prns_host_announce(server, byte_view(destination_hash, sizeof(destination_hash)), NULL, &command) == PRNS_STATUS_OK, "announce submission failed");
            REQUIRE(wait_outcome(&command, PRNS_COMMAND_OUTCOME_KIND_ANNOUNCED, NULL, 0, NULL), "announce failed");
            delay_millis(50);
        }
    }
    REQUIRE(routed, "announced destination did not become routable");
    REQUIRE(prns_host_establish_link(client, byte_view(destination_hash, sizeof(destination_hash)), &command) == PRNS_STATUS_OK, "link submission failed");
    REQUIRE(wait_outcome(&command, PRNS_COMMAND_OUTCOME_KIND_LINK_ESTABLISHED, link_id, sizeof(link_id), &command_value_length), "link establishment failed");
    REQUIRE(command_value_length == sizeof(link_id), "link ID length mismatch");

    REQUIRE(prns_host_request(client, byte_view(link_id, sizeof(link_id)), byte_view(path_hash, path_hash_length), byte_view(request_payload, request_payload_length), PRNS_RESPONSE_TIMEOUT_KIND_EXACT, fixture.timeout_millis, &command) == PRNS_STATUS_OK, "request submission failed");
    REQUIRE(next_application_event(events, PRNS_APPLICATION_EVENT_KIND_REQUEST, &event), "request event missing");
    REQUIRE(prns_event_bytes(event, PRNS_EVENT_FIELD_LINK_ID, &view) == PRNS_STATUS_OK && view.length == sizeof(request_link_id), "request link missing");
    memcpy(request_link_id, view.data, sizeof(request_link_id));
    REQUIRE(prns_event_bytes(event, PRNS_EVENT_FIELD_REQUEST_ID, &view) == PRNS_STATUS_OK && view.length == sizeof(request_id), "request ID missing");
    memcpy(request_id, view.data, sizeof(request_id));
    REQUIRE(prns_event_bytes(event, PRNS_EVENT_FIELD_DATA, &view) == PRNS_STATUS_OK && view.length == request_payload_length && memcmp(view.data, request_payload, request_payload_length) == 0, "request payload changed");
    REQUIRE(prns_event_u64(event, PRNS_EVENT_FIELD_RTT_MILLIS, &request_rtt_millis) == PRNS_STATUS_OK, "request RTT missing");
    prns_event_release(event);
    event = NULL;
    request_command = command;
    command = NULL;
    REQUIRE(prns_host_respond(server, byte_view(request_link_id, sizeof(request_link_id)), byte_view(request_id, sizeof(request_id)), request_rtt_millis, byte_view(response_payload, response_payload_length), &command) == PRNS_STATUS_OK, "response submission failed");
    REQUIRE(wait_outcome(&command, PRNS_COMMAND_OUTCOME_KIND_RESPONSE_SENT, NULL, 0, NULL), "response failed");
    command = request_command;
    request_command = NULL;
    REQUIRE(wait_outcome(&command, PRNS_COMMAND_OUTCOME_KIND_RESPONSE_RECEIVED, command_value, sizeof(command_value), &command_value_length), "request failed");
    REQUIRE(command_value_length == response_payload_length && memcmp(command_value, response_payload, response_payload_length) == 0, "response payload changed");

    REQUIRE(prns_host_set_link_resource_strategy(server, byte_view(request_link_id, sizeof(request_link_id)), PRNS_RESOURCE_STRATEGY_KIND_ACCEPT, fixture.maximum_uncompressed_bytes, fixture.accept_compressed, &command) == PRNS_STATUS_OK, "resource strategy submission failed");
    REQUIRE(wait_outcome(&command, PRNS_COMMAND_OUTCOME_KIND_RESOURCE_STRATEGY_SET, NULL, 0, NULL), "resource strategy failed");
    view = byte_view(metadata, metadata_length);
    REQUIRE(prns_host_begin_resource_upload(client, byte_view(link_id, sizeof(link_id)), (uint64_t)resource_payload_length, &view, PRNS_RESOURCE_COMPRESSION_KIND_NEVER, &upload) == PRNS_STATUS_OK, "resource upload creation failed");
    for (index = 0; index < fixture.chunk_count; index += 1) {
        PrnsStatus write_status;
        do {
            write_status = prns_resource_upload_write(upload, byte_view(chunks[index], chunk_lengths[index]));
            if (write_status == PRNS_STATUS_WOULD_BLOCK) {
                delay_millis(1);
            }
        } while (write_status == PRNS_STATUS_WOULD_BLOCK);
        REQUIRE(write_status == PRNS_STATUS_OK, "resource chunk write failed");
    }
    REQUIRE(prns_resource_upload_finish(upload, &command) == PRNS_STATUS_OK, "resource upload finish failed");
    REQUIRE(wait_outcome(&command, PRNS_COMMAND_OUTCOME_KIND_RESOURCE_SENT, NULL, 0, NULL), "resource send failed");
    prns_resource_upload_release(upload);
    upload = NULL;
    REQUIRE(next_application_event(events, PRNS_APPLICATION_EVENT_KIND_RESOURCE_AVAILABLE, &event), "resource event missing");
    REQUIRE(prns_event_bytes(event, PRNS_EVENT_FIELD_METADATA, &view) == PRNS_STATUS_OK && view.length == metadata_length && memcmp(view.data, metadata, metadata_length) == 0, "resource metadata changed");
    REQUIRE(prns_event_resource_stream(event, &resource_stream) == PRNS_STATUS_OK, "resource stream claim failed");
    prns_event_release(event);
    event = NULL;
    while (!finished) {
        REQUIRE(prns_resource_stream_next(resource_stream, 4, &view, &finished) == PRNS_STATUS_OK, "resource stream read failed");
        REQUIRE(received_resource_length + view.length <= sizeof(received_resource), "received resource too large");
        memcpy(received_resource + received_resource_length, view.data, view.length);
        received_resource_length += view.length;
    }
    REQUIRE(received_resource_length == resource_payload_length && memcmp(received_resource, resource_payload, resource_payload_length) == 0, "resource payload changed");
    prns_resource_stream_release(resource_stream);
    resource_stream = NULL;
    prns_event_stream_release(events);
    events = NULL;
    REQUIRE(prns_host_stop(client) == PRNS_STATUS_OK, "client stop failed");
    REQUIRE(prns_host_stop(server) == PRNS_STATUS_OK, "server stop failed");
    prns_host_release(client);
    client = NULL;
    prns_host_release(server);
    server = NULL;

    REQUIRE(create_host(server_root, 1, &fixture, announce_data, announce_data_length, argv[4], &restored_server), "restored server creation failed");
    REQUIRE(create_host(client_root, 0, &fixture, announce_data, announce_data_length, argv[4], &restored_client), "restored client creation failed");
    REQUIRE(prns_host_identity_hash(restored_server, &view) == PRNS_STATUS_OK && view.length == sizeof(server_identity) && memcmp(view.data, server_identity, sizeof(server_identity)) == 0, "server identity changed after restart");
    REQUIRE(prns_host_identity_hash(restored_client, &view) == PRNS_STATUS_OK && view.length == sizeof(client_identity) && memcmp(view.data, client_identity, sizeof(client_identity)) == 0, "client identity changed after restart");
    REQUIRE(prns_host_destination_hash(restored_server, 0, &view) == PRNS_STATUS_OK && view.length == sizeof(destination_hash) && memcmp(view.data, destination_hash, sizeof(destination_hash)) == 0, "destination changed after restart");
    REQUIRE(snapshot_has_route(restored_server, destination_hash, &restored) || restored, "server persistence did not restore");
    REQUIRE(restored, "server persistence did not report restoration");
    restored = 0;
    REQUIRE(snapshot_has_route(restored_client, destination_hash, &restored), "client route did not restore");
    REQUIRE(restored, "client persistence did not report restoration");
    result = 0;

cleanup:
    if (event != NULL) {
        prns_event_release(event);
    }
    if (resource_stream != NULL) {
        prns_resource_stream_release(resource_stream);
    }
    if (events != NULL) {
        prns_event_stream_release(events);
    }
    if (command != NULL) {
        prns_command_release(command);
    }
    if (request_command != NULL) {
        prns_command_release(request_command);
    }
    if (upload != NULL) {
        prns_resource_upload_abort(upload);
        prns_resource_upload_release(upload);
    }
    if (client != NULL) {
        prns_host_stop(client);
        prns_host_release(client);
    }
    if (server != NULL) {
        prns_host_stop(server);
        prns_host_release(server);
    }
    if (restored_client != NULL) {
        prns_host_stop(restored_client);
        prns_host_release(restored_client);
    }
    if (restored_server != NULL) {
        prns_host_stop(restored_server);
        prns_host_release(restored_server);
    }
    return result;
}
