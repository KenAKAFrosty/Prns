package rs.reticulum.prns.expo

import android.Manifest
import android.bluetooth.BluetoothManager
import android.content.Context
import android.content.pm.PackageManager
import android.location.LocationManager
import android.os.Build

/** Read-only OS prerequisites. The consuming app decides when to request access. */
object PrnsBluetoothEnvironment {
  fun permissions(): Array<String> = if (Build.VERSION.SDK_INT >= 31) {
    arrayOf(Manifest.permission.BLUETOOTH_SCAN, Manifest.permission.BLUETOOTH_CONNECT, Manifest.permission.BLUETOOTH_ADVERTISE)
  } else {
    arrayOf(Manifest.permission.ACCESS_FINE_LOCATION)
  }

  fun hasPermission(context: Context): Boolean = permissions().all {
    context.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED
  }

  fun ready(context: Context): Boolean = hasPermission(context) &&
    runCatching { context.getSystemService(BluetoothManager::class.java)?.adapter?.isEnabled == true }.getOrDefault(false) &&
    (Build.VERSION.SDK_INT >= 31 || context.getSystemService(LocationManager::class.java).isLocationEnabled)
}
