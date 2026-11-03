package rs.reticulum.prns.expo

import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import java.io.File

class PrnsHostModule : Module() {
  override fun definition() = ModuleDefinition {
    Name("PrnsHostPlatform")
    Constants("nativeImage" to PrnsNativeImage.name)
    AsyncFunction("defaultStoragePath") {
      val context = requireNotNull(appContext.reactContext) { "React context is unavailable" }
      val directory = File(context.applicationContext.filesDir, "prns")
      check(directory.isDirectory || directory.mkdirs()) { "Could not create PRNS storage" }
      directory.absolutePath
    }
  }
}
