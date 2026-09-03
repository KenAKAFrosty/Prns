import AsyncStorage from "@react-native-async-storage/async-storage";
import { act, renderHook, waitFor } from "@testing-library/react-native";
import { loadScaffoldState, SCAFFOLD_STORAGE_KEY } from "./scaffold-state";
import { ScaffoldStateProvider, useScaffoldState } from "./scaffold-state-context";

describe("ScaffoldStateProvider", () => {
  beforeEach(async () => {
    await AsyncStorage.removeItem(SCAFFOLD_STORAGE_KEY);
    jest.clearAllMocks();
  });

  it("persists generic scaffold updates", async () => {
    const { result } = renderHook(() => useScaffoldState(), {
      wrapper: ScaffoldStateProvider,
    });
    await waitFor(() => expect(result.current.status).toBe("ready"));

    await act(async () => {
      await result.current.updateScaffoldState((current) => ({
        ...current,
        showUnavailableFeatures: false,
      }));
    });

    expect(result.current.state.showUnavailableFeatures).toBe(false);
    await expect(loadScaffoldState()).resolves.toEqual(result.current.state);
  });

  it("removes persisted data and restores defaults", async () => {
    const { result } = renderHook(() => useScaffoldState(), {
      wrapper: ScaffoldStateProvider,
    });
    await waitFor(() => expect(result.current.status).toBe("ready"));

    await act(async () => {
      await result.current.resetDevelopmentData();
    });

    expect(result.current.state).toEqual({
      scaffoldSchema: 2,
      showUnavailableFeatures: true,
    });
    await expect(AsyncStorage.getItem(SCAFFOLD_STORAGE_KEY)).resolves.toBeNull();
  });
});
