using PersonalRns
using Sockets
using Test

host = Host(
    ephemeral_endpoint(
        required_capabilities=Capability[PersonalRns.CapabilityTcpClient],
    ),
)

try
    @test identity_hash(host) != PersonalRns.IdentityHash(zeros(UInt8, 16))
    @test backend_info(host).backend == PersonalRns.BackendKindNative
    initial_snapshot = snapshot(host)
    @test initial_snapshot.runtime.running
    @test initial_snapshot.runtime.interface_count == 0

    claim = claim_application_events(host)
    @test claim isa StreamClaimed
    @test claim_application_events(host) isa StreamAlreadyClaimed
    stream = claim.stream

    waiting = Threads.@spawn begin
        try
            next!(stream)
        catch failure
            failure
        end
    end
    sleep(0.02)
    interrupt_wait!(stream)
    interrupted = fetch(waiting)
    @test interrupted isa PersonalRns.StatusFailure
    @test interrupted.status == PersonalRns.StatusInterrupted
    close(stream)

    resource = send_resource(
        host,
        PersonalRns.LinkId(zeros(UInt8, 16)),
        Vector{UInt8}(codeunits("bounded upload"));
        compression=ResourceCompressionNever(),
    )
    @test resource isa CommandFailed
    @test resource.failure isa PersonalRns.CommandFailureUnknownLink

    attach = execute(
        host,
        HostCommandAttachInterface(
            InterfaceConfigTcpClient("127.0.0.1:9", BitrateAuto()),
        ),
    )
    attached = wait(attach)
    close(attach)
    @test attached isa CommandSucceeded
    @test attached.outcome isa PersonalRns.CommandOutcomeInterfaceAttached
    attached_snapshot = snapshot(host)
    @test attached_snapshot.runtime.interface_count == 1
    @test attached_snapshot.interfaces[1].interface_id == attached.outcome.interface

    detach = execute(
        host,
        HostCommandDetachInterface(attached.outcome.interface),
    )
    detached = wait(detach)
    close(detach)
    @test detached isa CommandSucceeded
    @test detached.outcome isa PersonalRns.CommandOutcomeInterfaceDetached
finally
    close(host)
end

function json_string(text, key)
    matched = match(Regex("\\\"$(key)\\\"\\s*:\\s*\\\"([^\\\"]*)\\\""), text)
    matched === nothing && error("missing string fixture field $(key)")
    matched.captures[1]
end

function json_strings(text, key)
    matched = match(Regex("\\\"$(key)\\\"\\s*:\\s*\\[([^]]*)\\]"), text)
    matched === nothing && error("missing array fixture field $(key)")
    map(split(matched.captures[1], ',')) do value
        String(strip(strip(value), '"'))
    end
end

function json_integer(text, key)
    matched = match(Regex("\\\"$(key)\\\"\\s*:\\s*(\\d+)"), text)
    matched === nothing && error("missing integer fixture field $(key)")
    parse(UInt64, matched.captures[1])
end

function json_boolean(text, key)
    matched = match(Regex("\\\"$(key)\\\"\\s*:\\s*(true|false)"), text)
    matched === nothing && error("missing boolean fixture field $(key)")
    matched.captures[1] == "true"
end

function successful_outcome(host, value)
    command = execute(host, value)
    try
        settlement = wait(command)
        settlement isa CommandSucceeded ||
            error("command failed with $(typeof(settlement.failure))")
        settlement.outcome
    finally
        close(command)
    end
end

function next_event(stream, event_type)
    while true
        event = next!(stream)
        event isa event_type && return event
    end
end

fixture_path = normpath(
    @__DIR__,
    "..",
    "..",
    "..",
    "conformance",
    "persistent-two-node-v2.json",
)
fixture = read(fixture_path, String)
@test json_integer(fixture, "schemaVersion") == PersonalRns.HOST_SCHEMA_VERSION
@test json_string(fixture, "compression") == "Never"

listener = listen(ip"127.0.0.1", 0)
port = Int(last(getsockname(listener)))
close(listener)

mktempdir() do root
    announce_data = hex2bytes(json_string(fixture, "announceAppDataHex"))
    destination = DestinationConfigSingle(
        DestinationName(
            json_string(fixture, "appName"),
            json_strings(fixture, "aspects"),
        ),
        DestinationIdentityConfigHostIdentity(),
        announce_data,
        RequestHandlerConfig[
            RequestHandlerConfig(
                json_string(fixture, "path"),
                RequestPolicyAllowAll,
            ),
        ],
    )
    server_options = persistent_endpoint(
        joinpath(root, "server"),
        DestinationConfig[destination];
        required_capabilities=Capability[PersonalRns.CapabilityTcpServer],
    )
    client_options = persistent_endpoint(
        joinpath(root, "client");
        required_capabilities=Capability[PersonalRns.CapabilityTcpClient],
    )
    server = Host(server_options)
    client = Host(client_options)
    server_identity = identity_hash(server)
    client_identity = identity_hash(client)
    destination_hash = only(destination_hashes(server))
    claim = claim_application_events(server)
    @test claim isa StreamClaimed
    stream = claim.stream

    try
        @test successful_outcome(
            server,
            HostCommandAttachInterface(
                InterfaceConfigTcpServer("127.0.0.1:$(port)", BitrateAuto()),
            ),
        ) isa PersonalRns.CommandOutcomeInterfaceAttached
        @test successful_outcome(
            client,
            HostCommandAttachInterface(
                InterfaceConfigTcpClient("127.0.0.1:$(port)", BitrateAuto()),
            ),
        ) isa PersonalRns.CommandOutcomeInterfaceAttached

        routed = false
        for _ in 1:50
            routed = any(
                route -> route.destination == destination_hash,
                snapshot(client).routes,
            )
            routed && break
            @test successful_outcome(
                server,
                HostCommandAnnounce(destination_hash, nothing),
            ) isa PersonalRns.CommandOutcomeAnnounced
            sleep(0.05)
        end
        @test routed

        link = successful_outcome(
            client,
            HostCommandEstablishLink(destination_hash),
        )
        @test link isa PersonalRns.CommandOutcomeLinkEstablished
        request_payload = hex2bytes(json_string(fixture, "payloadHex"))
        response_payload = hex2bytes(json_string(fixture, "responseHex"))
        request_command = execute(
            client,
            HostCommandRequest(
                link.link_id,
                RequestPathHash(
                    hex2bytes(json_string(fixture, "pathHashHex")),
                ),
                request_payload,
                ResponseTimeoutExact(json_integer(fixture, "timeoutMillis")),
            ),
        )
        request_task = @async wait(request_command)
        request = next_event(stream, ApplicationEventRequest)
        @test request.data == request_payload
        @test successful_outcome(
            server,
            HostCommandRespond(
                request.link_id,
                request.request_id,
                request.rtt_millis,
                response_payload,
            ),
        ) isa PersonalRns.CommandOutcomeResponseSent
        request_settlement = fetch(request_task)
        close(request_command)
        @test request_settlement isa CommandSucceeded
        @test request_settlement.outcome isa PersonalRns.CommandOutcomeResponseReceived
        @test request_settlement.outcome.data == response_payload

        @test successful_outcome(
            server,
            HostCommandSetLinkResourceStrategy(
                request.link_id,
                ResourceStrategyAccept(
                    json_integer(fixture, "maximumUncompressedBytes"),
                    json_boolean(fixture, "acceptCompressed"),
                ),
            ),
        ) isa PersonalRns.CommandOutcomeResourceStrategySet
        chunks = map(
            hex2bytes,
            json_strings(fixture, "chunksHex"),
        )
        resource_payload = reduce(vcat, chunks)
        metadata = hex2bytes(json_string(fixture, "metadataHex"))
        upload = begin_resource_upload(
            client,
            link.link_id,
            UInt64(length(resource_payload));
            packed_metadata=metadata,
            compression=ResourceCompressionNever(),
        )
        try
            foreach(chunk -> write!(upload, chunk), chunks)
            resource_command = finish!(upload)
            try
                resource_settlement = wait(resource_command)
                @test resource_settlement isa CommandSucceeded
                @test resource_settlement.outcome isa PersonalRns.CommandOutcomeResourceSent
            finally
                close(resource_command)
            end
        finally
            close(upload)
        end
        resource = next_event(stream, ApplicationEventResourceAvailable)
        @test resource.metadata == metadata
        received = UInt8[]
        try
            while true
                chunk = next!(resource.resource; maximum_bytes=4)
                chunk.finished && break
                append!(received, chunk.bytes)
            end
        finally
            close(resource.resource)
        end
        @test received == resource_payload
    finally
        close(stream)
    end

    stop!(client)
    stop!(server)
    close(client)
    close(server)

    restored_server = Host(server_options)
    restored_client = Host(client_options)
    try
        @test identity_hash(restored_server) == server_identity
        @test identity_hash(restored_client) == client_identity
        @test only(destination_hashes(restored_server)) == destination_hash
        @test snapshot(restored_server).persistence.restored
        restored_client_snapshot = snapshot(restored_client)
        @test restored_client_snapshot.persistence.restored
        @test any(
            route -> route.destination == destination_hash,
            restored_client_snapshot.routes,
        )
    finally
        close(restored_client)
        close(restored_server)
    end
end
