import { requireNativeModule } from "expo-modules-core";
import { DevelopmentNodeRuntime, PrimaryIdentityState, developmentRuntime } from "./index";

jest.mock("expo-modules-core", () => {
  const actual = jest.requireActual("expo-modules-core");
  return {
    ...actual,
    requireNativeModule: jest.fn((name: string) => {
      if (name === "PrnsApp") throw new Error("Prns native capability unavailable");
      return actual.requireNativeModule(name);
    }),
  };
});
jest.mock("@prns-internal/native-bindings/native", () => {
  throw new Error("the native player must not be imported without the platform capability");
});

test("SDK values remain usable without a native module or installed player", async () => {
  expect(requireNativeModule).not.toHaveBeenCalledWith("PrnsApp");
  expect(PrimaryIdentityState.Missing.new().tag).toBe("Missing");
  expect(DevelopmentNodeRuntime.Running).toBeDefined();
  await expect(developmentRuntime.readDevelopmentNodeSnapshot()).rejects.toThrow(
    "Prns native capability unavailable",
  );
});
