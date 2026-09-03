import { createDevelopmentRuntime } from "./facade";
import nativePrnsApp from "./native";

export { HOST_CONTRACT_FINGERPRINT, NATIVE_CONTRACT_FINGERPRINT } from "./contract.generated";
export type * as WireContract from "./contract.generated";
export {
  NativeBridgeError,
  NativeContractMismatchError,
  type DescribeRemoteControlTargetInput,
  type DevelopmentNodeSnapshot,
  type DevelopmentNodeStartOutcome,
  type DevelopmentNodeStopOutcome,
  type DevelopmentRuntime,
  type IdentityCreationOutcome,
  type IdentityImportPreviewOutcome,
  type InitiateRemoteControlPairingInput,
  type PrimaryIdentityState,
  type RemoteControlDescribeOutcome,
  type RemoteControlPairingCommandOutcome,
  type RemoteControlPairingDecisionInput,
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
export { NativePayloadError } from "./hydrate";

export const developmentRuntime = createDevelopmentRuntime(nativePrnsApp);

export const {
  approveRemoteControlPairing,
  createGeneratedIdentity,
  createImportedIdentity,
  describeRemoteControlTarget,
  inspectDevelopmentIdentity,
  initiateRemoteControlPairing,
  previewIdentityImport,
  readDevelopmentNodeSnapshot,
  rejectRemoteControlPairing,
  resetDevelopmentData,
  startDevelopmentNode,
  stopDevelopmentNode,
} = developmentRuntime;
