# JNI entry points are name-bound, including callbacks owned by the BLE pumps.
-keep class rs.reticulum.prns.app.expo.PrnsNative { *; }
-keep class rs.reticulum.prns.app.expo.PrnsBluetoothNative { *; }

# Generated JNA interfaces/structure fields are discovered by reflection.
-keep class rs.reticulum.prns.app.bindings.** { *; }
-keep class com.sun.jna.** { *; }
-keep class * extends com.sun.jna.Structure { public *; }
-dontwarn java.awt.**
