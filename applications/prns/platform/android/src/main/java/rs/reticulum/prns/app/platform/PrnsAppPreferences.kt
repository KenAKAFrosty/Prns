package rs.reticulum.prns.app.platform

import android.content.SharedPreferences
import java.util.Base64
import rs.reticulum.prns.host.bindings.validateBluetoothDiscoveryGroups

/** Stop removes restoration authority, while preserving the user's transport choice. */
internal class PrnsAppPreferences(
  private val preferences: SharedPreferences,
  private val validateGroups: (List<String>) -> List<String> = ::validateBluetoothDiscoveryGroups,
) {
  fun savedInput(): PrnsAppInput? {
    val stored = preferences.getString("startup", null) ?: return null
    // Unknown saved bytes are an explicit restoration failure, never Create.
    return PrnsAppInput.decode(Base64.getDecoder().decode(stored))
  }

  fun bluetoothSelected(): Boolean = preferences.getBoolean("bluetooth-selection", false)

  fun saveInput(input: PrnsAppInput) {
    val reopen = input.forRestoration()
    check(preferences.edit()
      .putString("startup", Base64.getEncoder().encodeToString(reopen.encode()))
      .putBoolean("bluetooth-selection", reopen.bluetooth).commit()) {
      "Could not save PRNS restoration selection"
    }
  }

  fun setBluetoothSelected(enabled: Boolean) {
    val editor = preferences.edit().putBoolean("bluetooth-selection", enabled)
    savedInput()?.let { editor.putString("startup", Base64.getEncoder().encodeToString(it.copy(bluetooth = enabled).encode())) }
    check(editor.commit()) { "Could not save Bluetooth selection" }
  }

  fun discoveryGroups(): List<String> {
    val encoded = preferences.getString("bluetooth-groups-v2", null) ?: return listOf("reticulum")
    require(encoded.length <= 800) { "Saved Bluetooth discovery groups are invalid" }
    val groups = encoded.split('.').map { Charsets.UTF_8.newDecoder().decode(java.nio.ByteBuffer.wrap(Base64.getDecoder().decode(it))).toString() }
    return validateGroups(groups)
  }

  fun saveDiscoveryGroups(groups: List<String>) {
    // Names may contain newlines; Base64 per name keeps persistence lossless.
    val validated = validateGroups(groups)
    check(preferences.edit().putString("bluetooth-groups-v2", validated.joinToString(".") {
      Base64.getEncoder().encodeToString(it.toByteArray(Charsets.UTF_8))
    }).commit()) { "Could not save Bluetooth discovery groups" }
  }

  fun reset() {
    check(preferences.edit().remove("startup").remove("bluetooth-selection")
      .remove("bluetooth-groups-v2").commit()) { "Could not clear PRNS platform preferences" }
  }

  fun clearInput() {
    check(preferences.edit().remove("startup").commit()) {
      "Could not clear PRNS restoration selection"
    }
  }
}
