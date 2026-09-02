import { createDevelopmentRuntime } from "./facade";
import nativePrnsApp from "./native";

export { NATIVE_CONTRACT_FINGERPRINT } from "./contract.generated";
export type * as WireContract from "./contract.generated";
export {
  NativeBridgeError,
  NativeContractMismatchError,
  type DescribeRemoteControlTargetInput,
  type DevelopmentNodeSnapshot,
  type DevelopmentNodeStartOutcome,
  type DevelopmentNodeStopOutcome,
  type DevelopmentRuntime,
  type InitiateRemoteControlPairingInput,
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
  describeRemoteControlTarget,
  initiateRemoteControlPairing,
  readDevelopmentNodeSnapshot,
  rejectRemoteControlPairing,
  resetDevelopmentData,
  startDevelopmentNode,
  stopDevelopmentNode,
} = developmentRuntime;
