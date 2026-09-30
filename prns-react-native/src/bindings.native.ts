import "@ubjs/react-native";
import { requireNativeModule } from "expo-modules-core";
import bindings, { bindingContract } from "./generated/prns_host_uniffi";
import { NATIVE_IMAGE } from "./generated/native-image.generated";
import { HOST_SEMANTIC_FINGERPRINT } from "./generated/contract.generated";
import { REMOTE_CONTROL_SEMANTIC_FINGERPRINT } from "personal-rns/remote-control";

const platform = requireNativeModule<{ nativeImage: string }>("PrnsHostPlatform");
if (platform.nativeImage !== NATIVE_IMAGE) {
  throw new Error(`PRNS native image mismatch: JS requires ${NATIVE_IMAGE}, installed ${platform.nativeImage}`);
}
bindings.initialize();
const contract = bindingContract();
if (contract.semanticFingerprint !== HOST_SEMANTIC_FINGERPRINT) {
  throw new Error("PRNS host contract mismatch; rebuild the native application");
}
if (contract.remoteControlFingerprint !== REMOTE_CONTROL_SEMANTIC_FINGERPRINT) {
  throw new Error("PRNS RemoteControl contract mismatch; rebuild the native application");
}
export * from "./generated/prns_host_uniffi";
export { default } from "./generated/prns_host_uniffi";
