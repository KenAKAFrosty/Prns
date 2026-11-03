package rs.reticulum.prns.app.platform

import android.content.SharedPreferences
import java.lang.reflect.Proxy
import org.junit.Assert.*
import org.junit.Test

class PrnsAppPreferencesTest {
  @Test fun stopPreservesExplicitBluetoothChoiceButRemovesRestorationInput() {
    val memory = MemoryPreferences()
    val selection = PrnsAppPreferences(memory.preferences)
    assertFalse(selection.bluetoothSelected())
    assertNull(selection.savedInput())
    for (bluetooth in listOf(true, false)) {
      selection.saveInput(PrnsAppInput(create = true, bluetooth = bluetooth))
      assertEquals(PrnsAppInput(create = false, bluetooth = bluetooth), selection.savedInput())
      selection.clearInput()
      // Read through a new owner to prove this is the persisted choice.
      val reopened = PrnsAppPreferences(memory.preferences)
      assertNull(reopened.savedInput())
      assertEquals(bluetooth, reopened.bluetoothSelected())
    }
  }

  @Test fun failedClearIsReportedAndCannotBeClaimedAsSuccessfulStopPersistence() {
    val memory = MemoryPreferences()
    val selection = PrnsAppPreferences(memory.preferences)
    selection.saveInput(PrnsAppInput(create = false, bluetooth = true))
    memory.commitSucceeds = false
    try { selection.clearInput(); fail("Failed commit was accepted") }
    catch (_: IllegalStateException) { }
    assertNotNull(selection.savedInput())
    assertTrue(selection.bluetoothSelected())
  }

  @Test fun liveSelectionUpdatesRestorationAndFailedWritesPreservePriorChoice() {
    val memory = MemoryPreferences()
    val selection = PrnsAppPreferences(memory.preferences)
    selection.saveInput(PrnsAppInput(create = true, bluetooth = false))
    selection.setBluetoothSelected(true)
    assertEquals(PrnsAppInput(create = false, bluetooth = true), selection.savedInput())
    memory.commitSucceeds = false
    assertTrue(runCatching { selection.setBluetoothSelected(false) }.isFailure)
    assertTrue(selection.bluetoothSelected())
    assertEquals(PrnsAppInput(create = false, bluetooth = true), selection.savedInput())
  }

  @Test fun discoveryNamesRoundTripAndValidationOrCommitFailureCannotReplaceThem() {
    val memory = MemoryPreferences()
    val validate: (List<String>) -> List<String> = { names -> require(names.none { it.isEmpty() }); names.sorted() }
    val selection = PrnsAppPreferences(memory.preferences, validate)
    val names = listOf("réticulum", "line\nwith.dot")
    selection.saveDiscoveryGroups(names)
    assertEquals(names.sorted(), PrnsAppPreferences(memory.preferences, validate).discoveryGroups())
    assertTrue(runCatching { selection.saveDiscoveryGroups(listOf("")) }.isFailure)
    memory.commitSucceeds = false
    assertTrue(runCatching { selection.saveDiscoveryGroups(listOf("other")) }.isFailure)
    assertEquals(names.sorted(), selection.discoveryGroups())
  }

  @Test fun malformedSavedUtf8IsNotSilentlyChangedIntoAnotherDiscoveryName() {
    val memory = MemoryPreferences()
    memory.preferences.edit().putString("bluetooth-groups-v2", "/w==").commit()
    val selection = PrnsAppPreferences(memory.preferences) { it }
    assertTrue(runCatching { selection.discoveryGroups() }.isFailure)
  }

  @Test fun resetClearsOnlyOwnedSelectionsAndReportsFailedWrites() {
    val memory = MemoryPreferences()
    val selection = PrnsAppPreferences(memory.preferences) { it }
    selection.saveInput(PrnsAppInput(create = false, bluetooth = true))
    selection.saveDiscoveryGroups(listOf("private"))
    memory.preferences.edit().putBoolean("notifications-requested", true).commit()
    memory.commitSucceeds = false
    assertTrue(runCatching { selection.reset() }.isFailure)
    assertNotNull(selection.savedInput())
    memory.commitSucceeds = true
    selection.reset()
    assertNull(selection.savedInput())
    assertFalse(selection.bluetoothSelected())
    assertEquals(listOf("reticulum"), selection.discoveryGroups())
    assertTrue(memory.preferences.getBoolean("notifications-requested", false))
  }

  /** The production Android interface, with only the exercised persistence calls. */
  private class MemoryPreferences {
    private val values = mutableMapOf<String, Any>()
    var commitSucceeds = true
    val preferences = Proxy.newProxyInstance(
      SharedPreferences::class.java.classLoader, arrayOf(SharedPreferences::class.java)
    ) { _, method, arguments ->
      val args = arguments ?: emptyArray()
      when (method.name) {
        "getString", "getBoolean" -> values[args[0] as String] ?: args[1]
        "edit" -> editor()
        else -> error("Unexpected preferences call: ${method.name}")
      }
    } as SharedPreferences

    private fun editor(): SharedPreferences.Editor {
      val pending = mutableMapOf<String, Any?>()
      return Proxy.newProxyInstance(
        SharedPreferences.Editor::class.java.classLoader, arrayOf(SharedPreferences.Editor::class.java)
      ) { proxy, method, arguments ->
        val args = arguments ?: emptyArray()
        when (method.name) {
          "putString", "putBoolean" -> { pending[args[0] as String] = args[1]; proxy }
          "remove" -> { pending[args[0] as String] = null; proxy }
          "commit" -> {
            if (commitSucceeds) pending.forEach { (key, value) ->
              if (value == null) values.remove(key) else values[key] = value
            }
            commitSucceeds
          }
          else -> error("Unexpected editor call: ${method.name}")
        }
      } as SharedPreferences.Editor
    }
  }
}
