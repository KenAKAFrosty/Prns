package rs.reticulum.prns.app.expo

/**
 * Admission only: the existing lifecycle executor still serializes native work.
 *
 * A start remains current until completion or cancellation. Check its ticket
 * both when receiving the service intent and when entering the queued worker,
 * so a Stop/Reset between those points cannot resurrect the runtime.
 */
internal class PrnsRuntimeAdmission<Owner : Any> {
  private var epoch = 0L
  private var nextTicket = 0L
  private val pending = linkedMapOf<Long, Long>()
  private var owner: Owner? = null

  @Synchronized
  fun enqueueStart(): Long {
    nextTicket = Math.addExact(nextTicket, 1L)
    pending[nextTicket] = epoch
    return nextTicket
  }

  @Synchronized
  fun isStartCurrent(ticket: Long): Boolean = pending[ticket] == epoch

  @Synchronized
  fun completeStart(ticket: Long): Boolean = pending.remove(ticket) != null

  /** Returns pending request IDs so the caller can reject their promises. */
  @Synchronized
  fun invalidateStarts(): List<Long> {
    epoch = Math.addExact(epoch, 1L)
    val cancelled = pending.keys.toList()
    pending.clear()
    return cancelled
  }

  /** Equal values are not interchangeable owners; identity must match. */
  @Synchronized
  fun reserveOwner(candidate: Owner): Boolean {
    if (owner == null) owner = candidate
    return owner === candidate
  }

  @Synchronized
  fun currentOwner(): Owner? = owner

  /** A failed drain retains the old owner, even after Android destroys it. */
  @Synchronized
  fun releaseOwner(candidate: Owner, drained: Boolean): Boolean {
    if (!drained || owner !== candidate) return false
    owner = null
    return true
  }
}
