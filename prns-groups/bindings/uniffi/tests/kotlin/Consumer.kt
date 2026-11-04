import java.nio.file.Files
import prns.checksum.regression.verifyChecksumCarriers
import kotlinx.coroutines.runBlocking
import uniffi.prns_groups_uniffi.GroupClose
import uniffi.prns_groups_uniffi.GroupLocalOwner
import rs.reticulum.prns.identities.keys.KeyOwner
import uniffi.prns_groups_core.GroupClockBounds
import uniffi.prns_groups_core.GroupClockReading
import uniffi.prns_groups.GroupExecutionContext
import uniffi.prns_groups.GroupIntent
import uniffi.prns_groups.GroupOriginalScope
import uniffi.prns_groups.GroupOutcome
import uniffi.prns_groups.GroupPageLimits
import uniffi.prns_groups.GroupRequest
import uniffi.prns_groups.createRequestId
import uniffi.prns_groups.defaultGroupProfile

// A real foreign owner, ordinary canonical client and retained cleanup observer.
// Domain behavior has deeper Rust/Swift coverage; this also compiles every
// selected generated namespace and distinguishes shutdown from AutoCloseable.
fun main() = runBlocking {
    verifyChecksumCarriers()
    val directory = Files.createTempDirectory("prns-group-kotlin-")
    try {
        KeyOwner.createNew(directory.resolve("key").toString()).use { keys ->
            keys.lease().use { lease ->
                GroupLocalOwner.createNew(directory.resolve("groups.redb").toString(), lease, 8u, 1048576uL).use { owner ->
                    owner.client().use { client ->
                        check(client.identity().contentEquals(lease.identityHash()))
                        val query = GroupPageLimits(8u, 1048576uL)
                        val initial = client.conversations(null, query)
                        check(initial.items.isEmpty() && initial.next == null)
                        val now = (System.currentTimeMillis() / 1000).toULong()
                        val context = GroupExecutionContext(GroupClockReading.Trusted(GroupClockBounds(now, now + 30uL)))
                        val request = GroupRequest(createRequestId(), GroupIntent.Create("JVM owner", defaultGroupProfile()), null)
                        val result = client.execute(request, context) as GroupOutcome.Created
                        val group = result.v1
                        val summary = client.conversations(null, query)
                        check(summary.items.size == 1 && summary.next == null)
                        val detail = client.conversation(group, query)
                        val members = client.members(group, null, query)
                        check(members.members.size == 1 && members.next == null &&
                            members.control.head.contentEquals(detail.control.head) && members.control.era.contentEquals(detail.control.era))
                        val scope = client.groupScope(group) as GroupOriginalScope.Control
                        check(scope.control.head.contentEquals(detail.control.head) && scope.control.era.contentEquals(detail.control.era) && scope.incarnation.size == 24)
                        val invite = GroupRequest(createRequestId(), GroupIntent.Invite(group, ByteArray(16) { 6 }), scope)
                        check(client.execute(invite, context) is GroupOutcome.Invited)
                        val invitations = client.invitations(now, null, query)
                        check(invitations.items.size == 1 && invitations.next == null)
                        val cleanup: GroupClose = owner.shutdown()
                        cleanup.use { it.wait() }
                    }
                }
            }
        }
        println("GROUP_KOTLIN_CONSUMER_OK")
    } finally {
        directory.toFile().deleteRecursively()
    }
}
