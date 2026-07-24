using PersonalRns
using Test

host = Host(
    ephemeral_endpoint(
        required_capabilities=Capability[PersonalRns.CapabilityTcpClient],
    ),
)

try
    @test identity_hash(host) != PersonalRns.IdentityHash(zeros(UInt8, 16))

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

    attach = execute(
        host,
        HostCommandAttachTcpClient("127.0.0.1:9", BitrateAuto()),
    )
    attached = wait(attach)
    close(attach)
    @test attached isa CommandSucceeded
    @test attached.outcome isa PersonalRns.CommandOutcomeInterfaceAttached

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
