package rs.reticulum.prns.app.expo

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import rs.reticulum.prns.app.bindings.*

/** Actual generated Kotlin/JNA over the shared Rust image; no Service or BLE pumps. */
@RunWith(AndroidJUnit4::class)
class PrnsNativeInstrumentedTest {
  @Test(timeout = 180_000)
  fun generatedBindingsPreserveIdentityAndContactsAcrossNativeRestart() = runBlocking {
    withIsolatedRuntime("generated-identity") { root ->
      assertEquals(NativeStoragePreparationOutcome.Prepared, nativePrepareStorage(root.path))
      assertEquals(PrimaryIdentityState.Missing, nativeInspectIdentity(root.path))
      val created = nativeCreateGeneratedIdentity(root.path) as IdentityCreationOutcome.Created
      val identity = created.identityHash
      assertEquals(16, identity.size)
      assertEquals(IdentityCreationOutcome.AlreadyExists, nativeCreateGeneratedIdentity(root.path))
      assertArrayEquals(identity, (nativeInspectIdentity(root.path) as PrimaryIdentityState.Present).identityHash)
      val destination = ByteArray(16) { (it + 1).toByte() }
      assertTrue(createManualContact(CreateManualContactInput(destination, null, "Generated Kotlin smoke")) is ContactMutationOutcome.Saved)
      var previousGeneration: ULong? = null
      repeat(2) {
        assertTrue(PrnsNative.nativePrepareBluetooth())
        assertFalse(PrnsNative.nativePrepareBluetooth())
        val started = nativeStart(root.path, DevelopmentNodeStartInput(null)) as DevelopmentNodeStartOutcome.Started
        val snapshot = readSnapshot()
        assertEquals(DevelopmentNodeRuntime.RUNNING, snapshot.runtime)
        assertEquals(started.snapshot.generationId, snapshot.generationId)
        assertArrayEquals(identity, (snapshot.primaryIdentity as PrimaryIdentityState.Present).identityHash)
        assertTrue(snapshot.localHost is LocalHostState.Running)
        assertEquals(bindingContract().app, snapshot.contractFingerprint)
        previousGeneration?.let { assertNotEquals(it, snapshot.generationId) }
        previousGeneration = snapshot.generationId
        assertTrue(nativeStart(root.path, DevelopmentNodeStartInput(null)) is DevelopmentNodeStartOutcome.AlreadyRunning)
        assertFalse(PrnsNative.nativeReleaseBluetooth())
        assertArrayEquals(destination, (listContacts() as ContactListOutcome.Listed).contacts.single().destination)
        assertStopped(nativeStop())
        assertTrue(PrnsNative.nativeReleaseBluetooth())
        assertEquals(DevelopmentNodeRuntime.STOPPED, readSnapshot().runtime)
      }
    }
  }

  @Test(timeout = 180_000)
  fun generatedImportedIdentityDoesNotReplaceExistingCredentials() = runBlocking {
    withIsolatedRuntime("imported-identity") { root ->
      val credential = ByteArray(64) { 0x42 }
      val expected = byteArrayOf(219.toByte(),188.toByte(),97,172.toByte(),2,81,59,86,25,149.toByte(),120,173.toByte(),73,118,245.toByte(),86)
      for (length in listOf(0, 3, 63, 65)) {
        assertEquals(IdentityCreationOutcome.InvalidLength, nativeCreateImportedIdentity(root.path, ByteArray(length)))
        assertEquals(PrimaryIdentityState.Missing, nativeInspectIdentity(root.path))
      }
      val imported = nativeCreateImportedIdentity(root.path, credential) as IdentityCreationOutcome.Created
      assertArrayEquals(expected, imported.identityHash)
      assertEquals(IdentityCreationOutcome.AlreadyExists, nativeCreateImportedIdentity(root.path, ByteArray(64) { 0x43 }))
      assertArrayEquals(expected, (nativeInspectIdentity(root.path) as PrimaryIdentityState.Present).identityHash)
      assertTrue(PrnsNative.nativePrepareBluetooth())
      assertTrue(nativeStart(root.path, DevelopmentNodeStartInput(null)) is DevelopmentNodeStartOutcome.Started)
      assertArrayEquals(expected, (readSnapshot().primaryIdentity as PrimaryIdentityState.Present).identityHash)
      assertStopped(nativeStop())
      assertTrue(PrnsNative.nativeReleaseBluetooth())
    }
  }

  private suspend fun withIsolatedRuntime(name: String, exercise: suspend (File) -> Unit) {
    val instrumentation = InstrumentationRegistry.getInstrumentation()
    val context = instrumentation.context
    assertEquals("rs.reticulum.prns.app.expo.test", context.packageName)
    assertEquals(context.packageName, instrumentation.targetContext.packageName)
    val base = context.noBackupFilesDir.canonicalFile
    val root = File(base, "$name/prns/development").canonicalFile
    assertTrue(root.path.startsWith(base.path + File.separator))
    assertNull(PrnsAndroidRuntime.service)
    assertEquals(DevelopmentNodeRuntime.STOPPED, readSnapshot().runtime)
    assertStopped(nativeReset(root.path))
    try { exercise(root) }
    finally {
      assertStopped(nativeStop())
      assertTrue(PrnsNative.nativeReleaseBluetooth())
      assertStopped(nativeReset(root.path))
      assertFalse(root.exists())
    }
  }

  private fun assertStopped(outcome: DevelopmentNodeStopOutcome) {
    assertTrue(outcome is DevelopmentNodeStopOutcome.Stopped || outcome is DevelopmentNodeStopOutcome.AlreadyStopped)
  }
}
