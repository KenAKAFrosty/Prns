package rs.reticulum.prns.app.expo

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Real JNI/native-runtime smoke, without a Service, BLE pumps, PSM, peer or pairing.
 * All durable operations use the instrumentation APK's sandbox, never target app data.
 * Rust bounds startup/shutdown internally; the outer timeout bounds the test as a whole.
 */
@RunWith(AndroidJUnit4::class)
class PrnsNativeInstrumentedTest {
  @Test(timeout = 180_000)
  fun actualJniPreservesIdentityAndContactsAcrossANativeGenerationRestart() {
    withIsolatedRuntime("generated-identity") { root ->
      // Accessing this object loads the actual libprns_app.so, not a mock bridge.
      val appFingerprint = PrnsNative.nativeContractFingerprint()
      val hostFingerprint = PrnsNative.nativeHostContractFingerprint()
      assertTrue("Application fingerprint is present", appFingerprint.startsWith("prns-app-native/"))
      assertTrue("Host fingerprint is present", hostFingerprint.isNotBlank())
      assertEquals(appFingerprint, PrnsNative.nativeContractFingerprint())
      assertEquals(hostFingerprint, PrnsNative.nativeHostContractFingerprint())
      assertBridgeInvalid(call("notAnOperation"))
      assertBridgeInvalid(call("start", root.path, "{".toByteArray(Charsets.UTF_8)))
      assertBridgeInvalid(call("createManualContact", root.path, "{}".toByteArray(Charsets.UTF_8)))
      assertBridgeInvalid(call("previewIdentityImport", input = ByteArray(65_537)))
      assertEquals("invalidLength", call("previewIdentityImport", input = byteArrayOf(1, 2, 3)).getString("type"))
      assertEquals("missing", call("inspectIdentity", root.path).getString("type"))

      val created = call("createGeneratedIdentity", root.path)
      assertEquals("created", created.getString("type"))
      val identity = created.getJSONArray("identityHash").toString()
      assertEquals(16, created.getJSONArray("identityHash").length())
      assertIdentity(root.path, identity)
      assertEquals("alreadyExists", call("createGeneratedIdentity", root.path).getString("type"))

      val emptyContacts = call("listContacts", root.path)
      assertEquals("listed", emptyContacts.getString("type"))
      assertEquals(0, emptyContacts.getJSONArray("contacts").length())
      val destination = JSONArray((0 until 16).map { it + 1 })
      val contactInput = JSONObject().put("destination", destination)
        .put("identity", JSONObject.NULL).put("alias", "JNI smoke contact")
      val saved = call("createManualContact", root.path, contactInput.toString().toByteArray(Charsets.UTF_8))
      assertEquals("saved", saved.getString("type"))
      assertContact(root.path, destination)

      assertTrue("Reserve the first native Bluetooth bridge", PrnsNative.nativePrepareBluetooth())
      assertFalse("A second preparation cannot replace the owner", PrnsNative.nativePrepareBluetooth())
      val start = call("start", root.path, startInput())
      assertEquals("Node startup must not wait for a platform PSM", "started", start.getString("type"))
      val firstSnapshot = assertRunningSnapshot(appFingerprint, identity)
      val firstGeneration = firstSnapshot.getString("generationId")
      val duplicate = call("start", root.path, startInput())
      assertEquals("alreadyRunning", duplicate.getString("type"))
      assertEquals(firstGeneration, duplicate.getJSONObject("snapshot").getString("generationId"))
      assertFalse("An attached bridge cannot be released while its worker lives", PrnsNative.nativeReleaseBluetooth())
      assertFalse("An attached bridge cannot be prepared twice", PrnsNative.nativePrepareBluetooth())

      assertStopped(call("stop"))
      assertTrue("Release only after the native generation drains", PrnsNative.nativeReleaseBluetooth())
      assertEquals("stopped", call("snapshot").getString("runtime"))
      assertIdentity(root.path, identity)
      assertContact(root.path, destination)

      assertTrue("Reserve a fresh bridge after stop", PrnsNative.nativePrepareBluetooth())
      assertEquals("started", call("start", root.path, startInput()).getString("type"))
      val restarted = assertRunningSnapshot(appFingerprint, identity)
      assertNotEquals("Restart creates a new generation, not another owner in parallel", firstGeneration, restarted.getString("generationId"))
      assertContact(root.path, destination)
      assertEquals(hostFingerprint, PrnsNative.nativeHostContractFingerprint())
      assertStopped(call("stop"))
      assertTrue(PrnsNative.nativeReleaseBluetooth())
    }
  }

  @Test(timeout = 180_000)
  fun actualJniImportsIdentityWithoutReplacementAndPreservesItAcrossRestart() {
    withIsolatedRuntime("imported-identity") { root ->
      // Public deterministic credentials, never an exported user identity.
      // Hash independently derived using pinned Python RNS 1.5.2.
      val credential = ByteArray(64) { 0x42.toByte() }
      val identity = JSONArray("[219,188,97,172,2,81,59,86,25,149,120,173,73,118,245,86]").toString()
      val fingerprint = PrnsNative.nativeContractFingerprint()
      assertEquals("missing", call("inspectIdentity", root.path).getString("type"))

      for (length in listOf(0, 3, 63, 65)) {
        val malformed = ByteArray(length) { 0x42.toByte() }
        assertEquals("invalidLength", call("previewIdentityImport", input = malformed).getString("type"))
        assertEquals("invalidLength", call("createImportedIdentity", root.path, malformed).getString("type"))
        assertEquals("Malformed input must not create an identity", "missing", call("inspectIdentity", root.path).getString("type"))
      }

      val preview = call("previewIdentityImport", input = credential)
      assertEquals("valid", preview.getString("type"))
      assertEquals(identity, preview.getJSONArray("identityHash").toString())
      assertEquals("Preview must not persist the credential", "missing", call("inspectIdentity", root.path).getString("type"))

      val imported = call("createImportedIdentity", root.path, credential)
      assertEquals("created", imported.getString("type"))
      assertEquals(identity, imported.getJSONArray("identityHash").toString())
      assertIdentity(root.path, identity)
      assertEquals("alreadyExists", call("createImportedIdentity", root.path, credential).getString("type"))

      val replacement = ByteArray(64) { 0x43.toByte() }
      val replacementPreview = call("previewIdentityImport", input = replacement)
      assertEquals("valid", replacementPreview.getString("type"))
      assertNotEquals(identity, replacementPreview.getJSONArray("identityHash").toString())
      assertEquals("alreadyExists", call("createImportedIdentity", root.path, replacement).getString("type"))
      assertIdentity(root.path, identity)

      var previousGeneration: String? = null
      repeat(2) {
        assertTrue("Reserve the native bridge for the imported identity", PrnsNative.nativePrepareBluetooth())
        assertEquals("started", call("start", root.path, startInput()).getString("type"))
        val snapshot = assertRunningSnapshot(fingerprint, identity)
        val generation = snapshot.getString("generationId")
        if (previousGeneration != null) assertNotEquals(previousGeneration, generation)
        previousGeneration = generation
        assertStopped(call("stop"))
        assertTrue("Release the bridge after the imported node drains", PrnsNative.nativeReleaseBluetooth())
        assertEquals("stopped", call("snapshot").getString("runtime"))
        assertIdentity(root.path, identity)
        assertEquals("Restart must not enable identity replacement", "alreadyExists", call("createImportedIdentity", root.path, replacement).getString("type"))
        assertIdentity(root.path, identity)
      }
    }
  }

  private fun withIsolatedRuntime(testName: String, exercise: (File) -> Unit) {
    val instrumentation = InstrumentationRegistry.getInstrumentation()
    val testContext = instrumentation.context
    val targetContext = instrumentation.targetContext
    // Android library tests are self-instrumenting: both contexts belong to the
    // test harness. Refuse app instrumentation rather than assuming they differ.
    val testPackage = "rs.reticulum.prns.app.expo.test"
    assertEquals("Storage must belong to the dedicated test APK", testPackage, testContext.packageName)
    assertEquals("Only the self-instrumenting test harness is supported", testPackage, targetContext.packageName)
    assertEquals(testPackage, testContext.applicationInfo.packageName)
    val testBase = testContext.noBackupFilesDir.canonicalFile
    val root = File(testBase, "$testName/prns/development").canonicalFile
    assertEquals(File(testContext.applicationInfo.dataDir, "no_backup").canonicalFile, testBase)
    assertTrue("Test root must remain inside the test APK sandbox", root.path.startsWith(testBase.path + File.separator))
    assertNull("This test must not share an active platform service", PrnsAndroidRuntime.service)
    assertEquals("Refuse to interrupt an existing native runtime", "stopped", call("snapshot").getString("runtime"))

    // Rust rejects this reset if any native owner belongs to a different root.
    // Do not enter cleanup until the test has established exclusive ownership.
    assertStopped(call("reset", root.path))
    try {
      exercise(root)
    } finally {
      // Cancellation must still drain the one owner before deleting test data.
      // Never delete files directly or release a bridge after an incomplete stop.
      val interrupted = Thread.interrupted()
      try {
        val stopped = call("stop", checkCancellation = false)
        assertStopped(stopped)
        assertTrue("Final cleanup must release the drained bridge", PrnsNative.nativeReleaseBluetooth())
        assertStopped(call("reset", root.path, checkCancellation = false))
        assertFalse("Only the instrumentation root was removed", root.exists())
      } finally {
        if (interrupted) Thread.currentThread().interrupt()
      }
    }
  }

  private fun startInput(): ByteArray = "{\"developmentTcpTarget\":null}".toByteArray(Charsets.UTF_8)

  private fun call(operation: String, root: String? = null, input: ByteArray? = null, checkCancellation: Boolean = true): JSONObject {
    if (checkCancellation && Thread.currentThread().isInterrupted) throw InterruptedException("JNI smoke cancelled")
    return JSONObject(PrnsNative.nativeCall(operation, root, input))
  }

  private fun assertBridgeInvalid(result: JSONObject) {
    assertEquals("bridgeFailure", result.getString("type"))
    assertEquals("invalidInput", result.getString("kind"))
  }

  private fun assertStopped(result: JSONObject) {
    assertTrue("Native stop/reset must finish, received ${result.optString("type")}", result.getString("type") in setOf("stopped", "alreadyStopped"))
  }

  private fun assertIdentity(root: String, expected: String) {
    val inspected = call("inspectIdentity", root)
    assertEquals("present", inspected.getString("type"))
    assertEquals(expected, inspected.getJSONArray("identityHash").toString())
  }

  private fun assertContact(root: String, destination: JSONArray) {
    val listed = call("listContacts", root)
    assertEquals("listed", listed.getString("type"))
    val contacts = listed.getJSONArray("contacts")
    assertEquals(1, contacts.length())
    val contact = contacts.getJSONObject(0)
    assertEquals(destination.toString(), contact.getJSONArray("destination").toString())
    assertEquals("JNI smoke contact", contact.getString("alias"))
  }

  private fun assertRunningSnapshot(fingerprint: String, identity: String): JSONObject {
    val snapshot = call("snapshot")
    assertEquals(fingerprint, snapshot.getString("contractFingerprint"))
    assertEquals("running", snapshot.getString("runtime"))
    assertEquals(identity, snapshot.getJSONObject("primaryIdentity").getJSONArray("identityHash").toString())
    assertEquals("running", snapshot.getJSONObject("localHost").getString("type"))
    return snapshot
  }
}
