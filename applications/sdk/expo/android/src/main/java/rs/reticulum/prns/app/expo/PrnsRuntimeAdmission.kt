package rs.reticulum.prns.app.expo

/**
 * Admission only: the existing lifecycle executor still serializes native work.
 *
 * A start remains current until completion or cancellation. Recheck its ticket
 * and the active owner when entering the queued worker: Stop, startup failure,
 * or destruction can retire that owner after intent admission. A retained owner
 * may retry teardown, but cannot start or refresh until a replacement owns it.
 */
internal class PrnsRuntimeAdmission<Owner : Any> {
  private var epoch = 0L
  private var nextTicket = 0L
  private val pending = linkedMapOf<Long, Long>()
  private var owner: Owner? = null
  private var ownerRetiring = false

  @Synchronized
  fun enqueueStart(): Long {
    nextTicket = Math.addExact(nextTicket, 1L)
    pending[nextTicket] = epoch
    return nextTicket
  }

  @Synchronized
  fun isStartCurrent(ticket: Long): Boolean = pending[ticket] == epoch

  /** A current ticket cannot restart an owner whose teardown has already begun. */
  @Synchronized
  fun canStart(candidate: Owner, ticket: Long): Boolean =
    owner === candidate && !ownerRetiring && pending[ticket] == epoch

  @Synchronized
  fun isOwnerActive(candidate: Owner): Boolean = owner === candidate && !ownerRetiring

  @Synchronized
  fun completeStart(ticket: Long): Boolean = pending.remove(ticket) != null

  /** Retire the current owner and return pending IDs for promise cancellation. */
  @Synchronized
  fun beginStop(): List<Long> {
    epoch = Math.addExact(epoch, 1L)
    if (owner != null) ownerRetiring = true
    val cancelled = pending.keys.toList()
    pending.clear()
    return cancelled
  }

  /** Equal values are not interchangeable owners; identity must match. */
  @Synchronized
  fun reserveOwner(candidate: Owner): Boolean {
    if (owner == null) {
      owner = candidate
      ownerRetiring = false
    }
    return owner === candidate
  }

  /** Retirement is irreversible for this owner, including after a failed drain. */
  @Synchronized
  fun retireOwner(candidate: Owner): Boolean {
    if (owner !== candidate) return false
    ownerRetiring = true
    return true
  }

  @Synchronized
  fun currentOwner(): Owner? = owner

  /** A failed drain retains the old owner, even after Android destroys it. */
  @Synchronized
  fun releaseOwner(candidate: Owner, drained: Boolean): Boolean {
    if (!drained || owner !== candidate) return false
    owner = null
    ownerRetiring = false
    return true
  }
}
