# Consumers must keep the uniffi-generated Kotlin and the JNA reflection paths
# reachable; minification would otherwise strip the classes the cdylib's JNI
# calls reach through.
-keep class uniffi.prns.** { *; }
-keep class com.sun.jna.** { *; }
-dontwarn com.sun.jna.**
