import { type ConfigPlugin, withGradleProperties } from "expo/config-plugins.js";

// The upstream Android BLE transport uses the API 29 L2CAP socket APIs.
const withAndroidRuntime: ConfigPlugin = (config) =>
  withGradleProperties(config, (configured) => {
    configured.modResults = configured.modResults.filter(
      (entry) =>
        entry.type !== "property" ||
        !["android.minSdkVersion", "reactNativeArchitectures", "org.gradle.jvmargs"].includes(
          entry.key,
        ),
    );
    configured.modResults.push({
      type: "property",
      key: "android.minSdkVersion",
      value: "29",
    });
    // One device ABI by default keeps local bring-up builds small. Gradle's
    // -PreactNativeArchitectures=x86_64 selects an Intel emulator explicitly.
    configured.modResults.push({
      type: "property",
      key: "reactNativeArchitectures",
      value: "arm64-v8a",
    });
    // Expo's dependency graph plus release lint exceed the template's 512 MB
    // metaspace cap. Bound workers separately instead of disabling release lint.
    configured.modResults.push({
      type: "property",
      key: "org.gradle.jvmargs",
      value: "-Xmx4096m -XX:MaxMetaspaceSize=1536m",
    });
    return configured;
  });

export default withAndroidRuntime;
