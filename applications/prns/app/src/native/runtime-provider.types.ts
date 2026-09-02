import type {
  DevelopmentRuntime,
  DevelopmentRuntimeScopeOptions,
  scopedDevelopmentRuntime,
} from "@prns-internal/expo";

export type RuntimeProvider =
  | {
      readonly availability: {
        readonly type: "available";
        readonly platform: "ios";
      };
      readonly runtime: DevelopmentRuntime;
      readonly acquire: (
        options: DevelopmentRuntimeScopeOptions,
      ) => ReturnType<typeof scopedDevelopmentRuntime>;
    }
  | {
      readonly availability: {
        readonly type: "unavailable";
        readonly platform: "android" | "web";
        readonly reason: "notImplemented";
      };
    };
