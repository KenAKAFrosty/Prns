import AsyncStorage from "@react-native-async-storage/async-storage";
import {
  DEFAULT_SCAFFOLD_STATE,
  loadScaffoldState,
  resetScaffoldState,
  SCAFFOLD_STORAGE_KEY,
  type ScaffoldState,
  saveScaffoldState,
} from "./scaffold-state";

const completedState: ScaffoldState = {
  ...DEFAULT_SCAFFOLD_STATE,
  showUnavailableFeatures: false,
};

describe("scaffold state storage", () => {
  beforeEach(async () => {
    await AsyncStorage.removeItem(SCAFFOLD_STORAGE_KEY);
    jest.clearAllMocks();
  });

  it("returns defaults when no state has been stored", async () => {
    await expect(loadScaffoldState()).resolves.toEqual(DEFAULT_SCAFFOLD_STATE);
  });

  it("saves and loads the single scaffold document", async () => {
    await saveScaffoldState(completedState);

    expect(AsyncStorage.setItem).toHaveBeenCalledWith(
      SCAFFOLD_STORAGE_KEY,
      JSON.stringify(completedState),
    );
    await expect(loadScaffoldState()).resolves.toEqual(completedState);
  });

  it("resets only the namespaced scaffold key", async () => {
    await saveScaffoldState(completedState);
    jest.clearAllMocks();

    await resetScaffoldState();

    expect(AsyncStorage.removeItem).toHaveBeenCalledWith(SCAFFOLD_STORAGE_KEY);
    expect(AsyncStorage.clear).not.toHaveBeenCalled();
    await expect(AsyncStorage.getItem(SCAFFOLD_STORAGE_KEY)).resolves.toBeNull();
  });

  it("clears an unsupported scaffold schema and returns defaults", async () => {
    const warning = jest.spyOn(console, "warn").mockImplementation(() => undefined);
    await AsyncStorage.setItem(
      SCAFFOLD_STORAGE_KEY,
      JSON.stringify({ ...completedState, scaffoldSchema: 1 }),
    );
    jest.clearAllMocks();

    await expect(loadScaffoldState()).resolves.toEqual(DEFAULT_SCAFFOLD_STATE);

    expect(AsyncStorage.removeItem).toHaveBeenCalledWith(SCAFFOLD_STORAGE_KEY);
    expect(warning).toHaveBeenCalledWith(expect.stringContaining("schema is unsupported"));
    warning.mockRestore();
  });

  it.each([
    ["malformed JSON", "{not-json"],
    [
      "an invalid document shape",
      JSON.stringify({ ...completedState, showUnavailableFeatures: "yes" }),
    ],
    ["an unexpected field", JSON.stringify({ ...completedState, unexpected: true })],
  ])("clears %s and returns defaults", async (_caseName, encoded) => {
    const warning = jest.spyOn(console, "warn").mockImplementation(() => undefined);
    await AsyncStorage.setItem(SCAFFOLD_STORAGE_KEY, encoded);
    jest.clearAllMocks();

    await expect(loadScaffoldState()).resolves.toEqual(DEFAULT_SCAFFOLD_STATE);

    expect(AsyncStorage.removeItem).toHaveBeenCalledWith(SCAFFOLD_STORAGE_KEY);
    expect(warning).toHaveBeenCalled();
    warning.mockRestore();
  });
});
