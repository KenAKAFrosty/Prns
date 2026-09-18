import { createDevelopmentRuntime } from "./facade";
import { createAccessorySetupRuntime } from "./accessory-setup";
import { createAndroidRuntime } from "./android-runtime";
import { getNativePrnsApp, nativeAccessorySetup, nativeAndroidRuntime } from "./native";

export {
  createAndroidRuntime,
  parseAndroidRuntimeStatus,
  type AndroidRuntime,
  type AndroidRuntimeStatus,
} from "./android-runtime";

export {
  createAccessorySetupRuntime,
  parseAccessorySetupStatus,
  type AccessorySetupPickerOutcome,
  type AccessorySetupRuntime,
  type AccessorySetupStatus,
  type AccessorySetupSubscription,
} from "./accessory-setup";
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
export const accessorySetupRuntime = createAccessorySetupRuntime(nativeAccessorySetup);
export const androidRuntime = createAndroidRuntime(nativeAndroidRuntime);

export const {
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
