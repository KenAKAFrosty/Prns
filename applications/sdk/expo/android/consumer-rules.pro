# JNI entry points are name-bound, including callbacks owned by the BLE pumps.
-keep class rs.reticulum.prns.app.expo.PrnsNative { *; }
-keep class rs.reticulum.prns.app.expo.PrnsBluetoothNative { *; }
