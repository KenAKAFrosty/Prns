package rs.reticulum.prns.app.platform

import rs.reticulum.prns.expo.PrnsForegroundService
import rs.reticulum.prns.expo.PrnsForegroundRuntime
import rs.reticulum.prns.expo.PrnsRuntimeDelegate

/** Android can reconstruct this owner before React exists. */
class PrnsAppService : PrnsForegroundService() {
  override val runtime: PrnsForegroundRuntime get() = PrnsAppProcess.runtime
  override fun createDelegate(): PrnsRuntimeDelegate = PrnsAppDelegate(applicationContext)
}
