import AsyncStorage from "@react-native-async-storage/async-storage";
import { act, renderHook, waitFor } from "@testing-library/react-native";
import { loadScaffoldState, SCAFFOLD_STORAGE_KEY } from "./scaffold-state";
import { ScaffoldStateProvider, useScaffoldState } from "./scaffold-state-context";

describe("ScaffoldStateProvider", () => {
  beforeEach(async () => {
    await AsyncStorage.removeItem(SCAFFOLD_STORAGE_KEY);
    jest.clearAllMocks();
  });

  it.each([
    ["import", "identity.imported-preview"],
    ["create", "identity.generated-preview"],
  ] as const)(
    "keeps the %s mode ephemeral and persists its preview fixture",
    async (mode, selectedIdentityFixtureId) => {
      const { result } = renderHook(() => useScaffoldState(), {
        wrapper: ScaffoldStateProvider,
      });
      await waitFor(() => expect(result.current.status).toBe("ready"));

      act(() => result.current.setOnboardingMode(mode));
      expect(result.current.onboardingMode).toBe(mode);
      expect(AsyncStorage.setItem).not.toHaveBeenCalled();

      await act(async () => {
        await result.current.completeOnboardingPreview(selectedIdentityFixtureId);
      });

      expect(result.current.onboardingMode).toBeNull();
      expect(result.current.state.onboardingPreview).toEqual({
        status: "completed",
        selectedIdentityFixtureId,
      });
      await expect(loadScaffoldState()).resolves.toEqual(result.current.state);
    },
  );

  it("persists generic updates and a skipped onboarding preview", async () => {
    const { result } = renderHook(() => useScaffoldState(), {
      wrapper: ScaffoldStateProvider,
    });
    await waitFor(() => expect(result.current.status).toBe("ready"));

    await act(async () => {
      await result.current.updateScaffoldState((current) => ({
        ...current,
        showUnavailableFeatures: false,
      }));
      await result.current.skipOnboardingPreview();
    });

    expect(result.current.state.showUnavailableFeatures).toBe(false);
    expect(result.current.state.onboardingPreview).toEqual({
      status: "skipped",
      selectedIdentityFixtureId: null,
    });
    await expect(loadScaffoldState()).resolves.toEqual(result.current.state);
  });

  it("removes persisted data and restores defaults", async () => {
    const { result } = renderHook(() => useScaffoldState(), {
      wrapper: ScaffoldStateProvider,
    });
    await waitFor(() => expect(result.current.status).toBe("ready"));

    act(() => result.current.setOnboardingMode("create"));
    await act(async () => {
      await result.current.completeOnboardingPreview("identity.generated-preview");
      await result.current.resetDevelopmentData();
    });

    expect(result.current.onboardingMode).toBeNull();
    expect(result.current.state.onboardingPreview.status).toBe("notStarted");
    expect(result.current.state.showUnavailableFeatures).toBe(true);
    await expect(AsyncStorage.getItem(SCAFFOLD_STORAGE_KEY)).resolves.toBeNull();
  });
});
