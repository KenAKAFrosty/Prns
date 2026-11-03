package rs.reticulum.prns.expo

/** Exact stop generation, meaningful only to the admission instance that issued it. */
class PrnsRuntimeStopOperation internal constructor(
  val cancelledStarts: List<Long>,
  val reset: Boolean,
)

/**
 * Admission only: the existing lifecycle executor still serializes native work.
 *
 * A start remains current until completion or cancellation. Recheck its ticket
 * and the active owner when entering the queued worker: Stop, startup failure,
 * or destruction can retire that owner after intent admission. A retained owner
 * may retry teardown, but cannot start or refresh until a replacement owns it.
 */
class PrnsRuntimeAdmission<Owner : Any> {
  private var epoch = 0L
  private var nextTicket = 0L
  private val pending = linkedMapOf<Long, Long>()
  private var owner: Owner? = null
  private var ownerRetiring = false
  private var pendingStop: PrnsRuntimeStopOperation? = null

  @Synchronized
  fun enqueueStart(): Long {
    nextTicket = Math.addExact(nextTicket, 1L)
    // A Start received during drain must not become valid if its Intent arrives later.
    pending[nextTicket] = if (pendingStop == null) epoch else Long.MIN_VALUE
    return nextTicket
  }

  @Synchronized
  fun isStartCurrent(ticket: Long): Boolean = pendingStop == null && pending[ticket] == epoch

  /** A current ticket cannot restart an owner whose teardown has already begun. */
  @Synchronized
  fun canStart(candidate: Owner, ticket: Long): Boolean =
    owner === candidate && !ownerRetiring && isStartCurrent(ticket)

  @Synchronized
  fun isOwnerActive(candidate: Owner): Boolean = owner === candidate && !ownerRetiring && pendingStop == null

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

  /** Fence replacement even when there is no Android service owner yet. */
  @Synchronized
  fun beginStopOperation(reset: Boolean = false): PrnsRuntimeStopOperation {
    // A newer Stop may supersede a queued Reset, but must not weaken its effect.
    val operation = PrnsRuntimeStopOperation(beginStop(), reset || pendingStop?.reset == true)
    pendingStop = operation
    return operation
  }

  @Synchronized
  fun isStopCurrent(operation: PrnsRuntimeStopOperation): Boolean = pendingStop === operation

  /** Native AND platform drain must be complete. A stale completion changes nothing. */
  @Synchronized
  fun completeStop(operation: PrnsRuntimeStopOperation, drained: Boolean): Boolean {
    if (!drained || pendingStop !== operation) return false
    pendingStop = null
    return true
  }

  /** Equal values are not interchangeable owners; identity must match. */
  @Synchronized
  fun reserveOwner(candidate: Owner): Boolean {
    if (owner == null) {
      if (pendingStop != null) return false
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

  /** No admitted Start, retained Stop or physical owner can still save state. */
  @Synchronized
  fun isIdle(): Boolean = owner == null && pendingStop == null && pending.isEmpty()

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
