import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  DEFAULT_SCAFFOLD_STATE,
  loadScaffoldState,
  resetScaffoldState,
  type ScaffoldState,
  saveScaffoldState,
} from "./scaffold-state";

export type ScaffoldStateStatus = "loading" | "ready";
export type ScaffoldStateUpdater = (current: ScaffoldState) => ScaffoldState;

export type ScaffoldStateContextValue = {
  readonly status: ScaffoldStateStatus;
  readonly state: ScaffoldState;
  readonly updateScaffoldState: (updater: ScaffoldStateUpdater) => Promise<ScaffoldState>;
  readonly resetDevelopmentData: () => Promise<void>;
};

const ScaffoldStateContext = createContext<ScaffoldStateContextValue | null>(null);

export type ScaffoldStateProviderProps = {
  readonly children: ReactNode;
};

function logLoadFailure(error: unknown): void {
  if (typeof __DEV__ !== "undefined" && __DEV__) {
    const detail = error instanceof Error ? error.message : "unknown storage error";
    console.warn(`[prns scaffold] Could not load development state: ${detail}.`);
  }
}

export function ScaffoldStateProvider({ children }: ScaffoldStateProviderProps) {
  const [status, setStatus] = useState<ScaffoldStateStatus>("loading");
  const [state, setState] = useState<ScaffoldState>(DEFAULT_SCAFFOLD_STATE);
  const stateRef = useRef<ScaffoldState>(DEFAULT_SCAFFOLD_STATE);
  const readyRef = useRef(false);
  const operationQueueRef = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    let active = true;

    void loadScaffoldState()
      .then((loaded) => {
        if (!active) {
          return;
        }
        stateRef.current = loaded;
        setState(loaded);
      })
      .catch((error: unknown) => {
        if (!active) {
          return;
        }
        logLoadFailure(error);
        stateRef.current = DEFAULT_SCAFFOLD_STATE;
        setState(DEFAULT_SCAFFOLD_STATE);
      })
      .finally(() => {
        if (!active) {
          return;
        }
        readyRef.current = true;
        setStatus("ready");
      });

    return () => {
      active = false;
      readyRef.current = false;
    };
  }, []);

  const enqueue = useCallback(function enqueueOperation<Result>(
    operation: () => Promise<Result>,
  ): Promise<Result> {
    const result = operationQueueRef.current.then(operation, operation);
    operationQueueRef.current = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }, []);

  const updateScaffoldState = useCallback(
    (updater: ScaffoldStateUpdater): Promise<ScaffoldState> => {
      if (!readyRef.current) {
        return Promise.reject(new Error("Scaffold state is not ready."));
      }

      return enqueue(async () => {
        const updated = updater(stateRef.current);
        await saveScaffoldState(updated);
        stateRef.current = updated;
        setState(updated);
        return updated;
      });
    },
    [enqueue],
  );

  const resetDevelopmentData = useCallback((): Promise<void> => {
    if (!readyRef.current) {
      return Promise.reject(new Error("Scaffold state is not ready."));
    }

    return enqueue(async () => {
      await resetScaffoldState();
      stateRef.current = DEFAULT_SCAFFOLD_STATE;
      setState(DEFAULT_SCAFFOLD_STATE);
    });
  }, [enqueue]);

  const value = useMemo<ScaffoldStateContextValue>(
    () => ({
      status,
      state,
      updateScaffoldState,
      resetDevelopmentData,
    }),
    [resetDevelopmentData, state, status, updateScaffoldState],
  );

  return <ScaffoldStateContext.Provider value={value}>{children}</ScaffoldStateContext.Provider>;
}

export function useScaffoldState(): ScaffoldStateContextValue {
  const context = useContext(ScaffoldStateContext);
  if (context === null) {
    throw new Error("useScaffoldState must be used within ScaffoldStateProvider.");
  }
  return context;
}
