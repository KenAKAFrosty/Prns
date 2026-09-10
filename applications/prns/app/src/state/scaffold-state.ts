import AsyncStorage from "@react-native-async-storage/async-storage";

export const SCAFFOLD_STORAGE_KEY = "rs.reticulum.prns.dev/scaffold";
export const SCAFFOLD_SCHEMA = 2 as const;

export type ScaffoldState = {
  readonly scaffoldSchema: typeof SCAFFOLD_SCHEMA;
  readonly showUnavailableFeatures: boolean;
};

export const DEFAULT_SCAFFOLD_STATE: ScaffoldState = {
  scaffoldSchema: SCAFFOLD_SCHEMA,
  showUnavailableFeatures: true,
};

const scaffoldStateKeys = ["scaffoldSchema", "showUnavailableFeatures"] as const;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasExactKeys(record: Record<string, unknown>, expected: readonly string[]): boolean {
  const actual = Object.keys(record);
  return actual.length === expected.length && expected.every((key) => actual.includes(key));
}

function isScaffoldState(value: unknown): value is ScaffoldState {
  return (
    isRecord(value) &&
    hasExactKeys(value, scaffoldStateKeys) &&
    value.scaffoldSchema === SCAFFOLD_SCHEMA &&
    typeof value.showUnavailableFeatures === "boolean"
  );
}

function logDiscardedState(reason: string): void {
  if (typeof __DEV__ !== "undefined" && __DEV__) {
    console.warn(`[prns scaffold] Discarding development state: ${reason}.`);
  }
}

async function discardInvalidState(reason: string): Promise<ScaffoldState> {
  logDiscardedState(reason);
  await resetScaffoldState();
  return DEFAULT_SCAFFOLD_STATE;
}

export async function loadScaffoldState(): Promise<ScaffoldState> {
  const encoded = await AsyncStorage.getItem(SCAFFOLD_STORAGE_KEY);
  if (encoded === null) {
    return DEFAULT_SCAFFOLD_STATE;
  }

  let decoded: unknown;
  try {
    decoded = JSON.parse(encoded);
  } catch {
    return discardInvalidState("stored JSON is malformed");
  }

  if (!isRecord(decoded) || decoded.scaffoldSchema !== SCAFFOLD_SCHEMA) {
    return discardInvalidState("the scaffold schema is unsupported");
  }

  if (!isScaffoldState(decoded)) {
    return discardInvalidState("the stored document has an invalid shape");
  }

  return decoded;
}

export async function saveScaffoldState(state: ScaffoldState): Promise<void> {
  if (!isScaffoldState(state)) {
    throw new TypeError("Cannot save an invalid scaffold-state document.");
  }

  await AsyncStorage.setItem(SCAFFOLD_STORAGE_KEY, JSON.stringify(state));
}

export async function resetScaffoldState(): Promise<void> {
  await AsyncStorage.removeItem(SCAFFOLD_STORAGE_KEY);
}
