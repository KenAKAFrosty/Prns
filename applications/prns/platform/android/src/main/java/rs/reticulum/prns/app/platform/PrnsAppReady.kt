package rs.reticulum.prns.app.platform

/** The local owner survives optional OS admission failure. Only Stop invalidates
 * this handoff; no network request may precede successful local readiness. */
internal suspend fun prnsAfterLocalReady(
  ready: suspend () -> Unit,
  current: () -> Boolean,
  admit: () -> Unit,
) {
  ready()
  check(current()) { "Start was cancelled by Stop" }
  admit()
}
