# Shared Android Bluetooth ownership

These Kotlin sources own GATT operation deadlines, callback ownership,
capability negotiation and bounded control-output decoding. They depend only on
Android Bluetooth and the JDK; tests use JUnit. They do not own an Expo runtime,
JNI image, host session or application lifecycle.

Consumers compile `src/main/java` into their Android module so the helpers retain
Kotlin `internal` visibility. Compile `src/test/java` with that module's unit
tests. Personal Hopspot includes these source sets directly. SDK packaging copies
these exact sources through its generated-output ownership contract, allowing a
published package to remain self-contained. Edit the files here, not a generated
SDK package projection. Existing Kotlin package names remain unchanged.
