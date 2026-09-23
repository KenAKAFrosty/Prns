import { createDevelopmentRuntime } from "./facade";
import { createBluetoothAuthorizationRuntime } from "./bluetooth-authorization";
import { createAndroidRuntime } from "./android-runtime";
import { getNativePrnsApp, nativeBluetoothAuthorization, nativeAndroidRuntime } from "./native";

export {
  createAndroidRuntime,
  parseAndroidRuntimeStatus,
  type AndroidRuntime,
  type AndroidRuntimeStatus,
} from "./android-runtime";

export {
  createBluetoothAuthorizationRuntime,
  parseBluetoothAuthorizationStatus,
  type BluetoothAuthorizationRuntime,
  type BluetoothAuthorizationStatus,
} from "./bluetooth-authorization";
export {
  NativeContractMismatchError,
  NativeStoragePreparationError,
  createDevelopmentRuntime,
  type DevelopmentRuntime,
} from "./facade";
export {
  DevelopmentRuntimeConfigurationError,
  DevelopmentRuntimeOperationError,
  DevelopmentRuntimeStartError,
  DevelopmentRuntimeStopError,
  makeEffectDevelopmentRuntime,
  scopedDevelopmentRuntime,
  type DevelopmentRuntimeFailure,
  type DevelopmentRuntimeScopeOptions,
  type DevelopmentRuntimeSession,
  type EffectDevelopmentRuntime,
} from "./effects";
export { NativePayloadError } from "./native-payload";

export const developmentRuntime = createDevelopmentRuntime(getNativePrnsApp);
export const bluetoothAuthorizationRuntime = createBluetoothAuthorizationRuntime(
  nativeBluetoothAuthorization,
);
export const androidRuntime = createAndroidRuntime(nativeAndroidRuntime);

export const {
  attachHost,
  announceLxmf,
  approveRemoteControlPairing,
  cancelLxmfMessage,
  createManualContact,
  createGeneratedIdentity,
  createImportedIdentity,
  describeRemoteControlTarget,
  deleteContact,
  getContact,
  inspectDevelopmentIdentity,
  initiateRemoteControlPairing,
  listLxmfMessages,
  listLxmfPeers,
  listContacts,
  measureLxmfText,
  previewIdentityImport,
  readDevelopmentNodeSnapshot,
  readRemoteNode,
  changeRemoteNode,
  startRemoteWifiTrial,
  inspectRemoteWifiTrial,
  finishRemoteWifiTrial,
  rejectRemoteControlPairing,
  retryLxmfMessage,
  resetDevelopmentData,
  saveObservedDestination,
  setContactAlias,
  setContactPinned,
  startDevelopmentNode,
  sendDirectText,
  stopDevelopmentNode,
} = developmentRuntime;

export * from "@prns-internal/native-bindings/values";

export {
  HOST_CONTRACT_FINGERPRINT,
  NATIVE_CONTRACT_FINGERPRINT,
} from "@prns-internal/native-bindings";
