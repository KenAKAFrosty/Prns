package rs.reticulum.prns.app.platform

import java.nio.file.Files
import java.nio.file.attribute.PosixFilePermissions
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import rs.reticulum.prns.chat.app.AppBootstrapException
import rs.reticulum.prns.chat.app.appOwner
import rs.reticulum.prns.chat.app.appPrepareExisting
import rs.reticulum.prns.chat.app.appPrepareReset
import rs.reticulum.prns.chat.app.appReset
import rs.reticulum.prns.chat.app.appStop
import rs.reticulum.prns.chat.app.appPrepareNew
import rs.reticulum.prns.chat.app.appShutdown
import uniffi.prns_service_runtime.ServiceRuntimeNetworkPhase
import uniffi.prns_service_runtime.ServiceRuntimeException

class PrnsAppStartTest {
  @Test fun localReadinessPrecedesOsAdmissionAndItsFailureIsPreserved() = runBlocking {
    val order = mutableListOf<String>()
    val denied = SecurityException("Foreground service admission refused")
    val failure = runCatching {
      prnsAfterLocalReady(ready = { order.add("local-ready") }, current = { true }) {
        order.add("os-admission")
        throw denied
      }
    }.exceptionOrNull()
    assertSame(denied, failure)
    assertEquals(listOf("local-ready", "os-admission"), order)
  }

  @Test fun stopWhileLocalReadinessFinishesPreventsOsAdmission() = runBlocking {
    var current = true
    var admitted = false
    val failure = runCatching {
      prnsAfterLocalReady(ready = { current = false }, current = { current }) { admitted = true }
    }.exceptionOrNull()
    assertTrue(failure is IllegalStateException)
    assertFalse(admitted)
  }

  /** Opt-in host-image conformance; ordinary Android JVM tests have no host dylib. */
  @Test fun realLocalOwnerRemainsReadableAfterInjectedOsAdmissionFailure() = runBlocking {
    assumeTrue(System.getProperty("prns.app.host-image-test") == "true")
    val root = Files.createDirectories(installationRoot)
    Files.setPosixFilePermissions(root, PosixFilePermissions.fromString("rwx------"))
    try {
      val owner = appPrepareNew(root.toString())
      val denied = SecurityException("Foreground service admission refused")
      val failure = runCatching {
        prnsAfterLocalReady(ready = { owner.ready() }, current = { true }) { throw denied }
      }.exceptionOrNull()
      assertSame(denied, failure)
      assertEquals(ServiceRuntimeNetworkPhase.OFFLINE, owner.networkStatus().phase)
      assertEquals(0uL, owner.networkStatus().generation)
      assertTrue(appOwner().identities().identities().isEmpty())
    } finally {
      withTimeout(5_000) { appReset(appPrepareReset()) }
      root.toFile().deleteRecursively()
    }
  }

  @Test fun failedLocalOpenStopsBeforeReviewedReset() = runBlocking {
    assumeTrue(System.getProperty("prns.app.host-image-test") == "true")
    val root = Files.createDirectories(installationRoot)
    Files.setPosixFilePermissions(root, PosixFilePermissions.fromString("rwx------"))
    try {
      appPrepareNew(root.toString()).ready()
      appShutdown().wait()
      val catalog = root.resolve("v2/identities/identities/catalog.bin")
      val corrupt = byteArrayOf(1, 2, 3)
      Files.write(catalog, corrupt)
      val owner = appPrepareExisting(root.toString())
      val failure = runCatching { withTimeout(5_000) { owner.ready() } }.exceptionOrNull()
      assertTrue(failure is ServiceRuntimeException.Identity)
      val review = appPrepareReset()
      val stop = appStop()
      withTimeout(5_000) { stop.wait() }
      assertEquals(ServiceRuntimeNetworkPhase.OFFLINE, stop.status().phase)
      assertArrayEquals(corrupt, Files.readAllBytes(catalog))
      withTimeout(5_000) { appReset(review) }
      assertFalse(Files.exists(root.resolve("v2")))
      assertTrue(runCatching { appOwner() }.exceptionOrNull() is AppBootstrapException.NotStarted)
    } finally {
      runCatching { withTimeout(5_000) { appReset(appPrepareReset()) } }
      root.toFile().deleteRecursively()
    }
  }


  private companion object {
    // The public process registry retains its private root even across a reset.
    val installationRoot by lazy { Files.createTempDirectory("prns-platform-installation-") }
  }

}
