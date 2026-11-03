import "./native-mock";
import AsyncStorage from "@react-native-async-storage/async-storage";
import { prns_groups as groups, prns_groups_core as protocol } from "@prns/app-bindings";
import type { prns_identities_core as core } from "@prns/app-bindings";
import { acknowledgeInput, captureInput, inputId, inputIntent, loadInput, releaseExpiredInput, submitInput, InputExpired, InputRejected, PendingInputConflict } from "../../src/messaging/input";
import { hex } from "../../src/bytes";

const data = new Map<string, string>();
const bytes = (size: number, value = 1) => new Uint8Array(size).fill(value).buffer;
const identity: core.IdentityReference = { id: { incarnation: bytes(16), sequence: 1n }, publicHash: bytes(16, 2) };
const context = { clock: new protocol.GroupClockReading.Unknown() };
const profile: groups.GroupProfile = { maxMembers: 256, maxRecordBodyBytes: 2054, maxPlaintextBytes: 16384,
  maxWireBytes: 65536, maxSnapshotBytes: 262144, maxEnrollmentFrameBytes: 524288 };
const intent = (title = "Café 👋") => new groups.GroupIntent.Create({ title, profile });
function operation(id: ArrayBuffer, value: groups.GroupIntent): groups.GroupOperation {
  return { request: { id, intent: value }, scope: new groups.GroupOperationScope.Create(),
    state: new groups.GroupOperationState.Completed(new groups.GroupOutcome.Created(bytes(32, 8))) };
}
function client(submit: (id: ArrayBuffer, value: groups.GroupIntent) => Promise<groups.GroupOperation> = jest.fn(async (id: ArrayBuffer, value: groups.GroupIntent) => operation(id, value))) {
  return { identity: jest.fn(async () => identity.publicHash), submit,
    operation: jest.fn<Promise<groups.GroupOperation | undefined>, [ArrayBuffer]>(async () => undefined) } as unknown as groups.ClientLike;
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => { resolve = yes; });
  return { promise, resolve };
}
beforeEach(() => {
  data.clear(); jest.clearAllMocks();
  jest.mocked(groups.createRequestId).mockReturnValue(bytes(32, 4));
  jest.mocked(AsyncStorage.getItem).mockImplementation(async (key) => data.get(key) ?? null);
  jest.mocked(AsyncStorage.setItem).mockImplementation(async (key, value) => { data.set(key, value); });
  jest.mocked(AsyncStorage.removeItem).mockImplementation(async (key) => { data.delete(key); });
});

test("public generated converter roundtrips canonical enums, nested endpoints and bigint", async () => {
  const values: groups.GroupIntent[] = [intent(), new groups.GroupIntent.RemoveMember({ group: bytes(32), member: { id: bytes(16), incarnation: 18446744073709551615n } }),
    new groups.GroupIntent.Publish({ group: bytes(32), namespace: 7, schema: 11, targets: new groups.GroupTargets.OtherMembers(), body: bytes(9) })];
  for (const value of values) {
    const draft = captureInput(identity, value);
    expect(inputIntent(draft)).toEqual(value);
    expect(hex(inputId(draft))).toBe(draft.id);
  }
});

test("failed save cannot admit native work; lost write result keeps exact saved input", async () => {
  const native = client();
  const input = captureInput(identity, intent());
  jest.mocked(AsyncStorage.setItem).mockRejectedValueOnce(new Error("disk"));
  await expect(submitInput(native, identity, input, context)).rejects.toThrow("disk");
  expect(native.submit).not.toHaveBeenCalled();
  jest.mocked(AsyncStorage.setItem).mockImplementationOnce(async (key, value) => { data.set(key, value); throw new Error("lost write reply"); });
  await expect(submitInput(native, identity, input, context)).rejects.toThrow("lost write reply");
  expect(native.submit).not.toHaveBeenCalled();
  expect(await loadInput(identity)).toEqual(input);
});

test("lost native reply survives reload and resumes exact request and input", async () => {
  const native = client(jest.fn().mockRejectedValueOnce(new Error("lost reply")).mockImplementation(async (id, value) => operation(id, value)));
  const input = captureInput(identity, intent());
  await expect(submitInput(native, identity, input, context)).rejects.toThrow("lost reply");
  const restored = (await loadInput(identity))!;
  await submitInput(native, identity, restored, context);
  expect(native.submit).toHaveBeenCalledTimes(2);
  expect(jest.mocked(native.submit).mock.calls[0]?.[0]).toEqual(jest.mocked(native.submit).mock.calls[1]?.[0]);
  expect(jest.mocked(native.submit).mock.calls[0]?.[1]).toEqual(jest.mocked(native.submit).mock.calls[1]?.[1]);
  expect(await loadInput(identity)).toBeUndefined();
  expect(groups.createRequestId).toHaveBeenCalledTimes(1);
});

test("different ID or different input cannot replace an unresolved form", async () => {
  const native = client(jest.fn().mockRejectedValue(new Error("uncertain")));
  const first = captureInput(identity, intent());
  await expect(submitInput(native, identity, first, context)).rejects.toThrow("uncertain");
  const changed = captureInput(identity, intent("Different"), inputId(first));
  const other = captureInput(identity, intent(), bytes(32, 9));
  await expect(submitInput(native, identity, changed, context)).rejects.toBeInstanceOf(PendingInputConflict);
  await expect(submitInput(native, identity, other, context)).rejects.toBeInstanceOf(PendingInputConflict);
  expect(native.submit).toHaveBeenCalledTimes(1);
  expect(await loadInput(identity)).toEqual(first);
});

test("late completion cannot erase a newer saved form, native waits do not hold storage", async () => {
  const late = deferred<groups.GroupOperation>();
  const first = captureInput(identity, intent());
  const native = client(jest.fn(() => late.promise));
  const submitted = submitInput(native, identity, first, context);
  while (jest.mocked(native.submit).mock.calls.length === 0) await Promise.resolve();
  await acknowledgeInput(first, operation(inputId(first), intent()));
  const second = captureInput(identity, intent("Next"), bytes(32, 9));
  await expect(submitInput(client(jest.fn().mockRejectedValue(new Error("uncertain"))), identity, second, context)).rejects.toThrow("uncertain");
  late.resolve(operation(inputId(first), intent()));
  await submitted;
  expect(await loadInput(identity)).toEqual(second);
});

test("identity mismatch rejects before storage or admission", async () => {
  const native = client();
  const input = captureInput(identity, intent());
  const replacement = { ...identity, id: { ...identity.id, sequence: 2n } };
  await expect(submitInput(native, replacement, input, context)).rejects.toThrow("different identity");
  jest.mocked(native.identity).mockResolvedValueOnce(bytes(16, 9));
  await expect(submitInput(native, identity, input, context)).rejects.toThrow("different identity");
  expect(AsyncStorage.setItem).not.toHaveBeenCalled(); expect(native.submit).not.toHaveBeenCalled();
});

test("older, future or damaged saved envelope is preserved and cannot be overwritten", async () => {
  const input = captureInput(identity, intent());
  const key = `prns.app.v1.group-input.${input.identity}`;
  for (const raw of ["not json", JSON.stringify({ ...input, version: 1 }), JSON.stringify({ ...input, version: 99 }), JSON.stringify({ ...input, intent: input.intent + "00" })]) {
    data.set(key, raw);
    await expect(loadInput(identity)).rejects.toThrow("kept unchanged");
    await expect(submitInput(client(), identity, input, context)).rejects.toThrow("kept unchanged");
    expect(data.get(key)).toBe(raw);
  }
});

test("only definite invalid input unlocks correction; failed cleanup retains it", async () => {
  const input = captureInput(identity, intent());
  const native = client(jest.fn().mockRejectedValue(new protocol.GroupApiError.InvalidInput()));
  await expect(submitInput(native, identity, input, context)).rejects.toBeInstanceOf(InputRejected);
  expect(await loadInput(identity)).toBeUndefined();
  jest.mocked(AsyncStorage.removeItem).mockRejectedValueOnce(new Error("uncertain cleanup"));
  await expect(submitInput(native, identity, input, context)).rejects.toThrow("uncertain cleanup");
  expect(await loadInput(identity)).toEqual(input);
  jest.mocked(native.submit).mockRejectedValue(new protocol.GroupApiError.Unavailable());
  await expect(submitInput(native, identity, input, context)).rejects.toBeInstanceOf(protocol.GroupApiError.Unavailable);
  expect(await loadInput(identity)).toEqual(input);
});

async function expiredInput(native: groups.ClientLike, input: ReturnType<typeof captureInput>): Promise<InputExpired> {
  jest.mocked(native.submit).mockRejectedValueOnce(new protocol.GroupApiError.Retry({ reason: protocol.GroupRetryError.Expired }));
  const failure: unknown = await submitInput(native, identity, input, context).catch((cause: unknown) => cause);
  if (!(failure instanceof InputExpired)) throw new Error("Expected a native expiry rejection");
  return failure;
}

test("native expiry retains exact input until explicit release without resubmission or a new ID", async () => {
  const native = client(); const input = captureInput(identity, intent());
  const expired = await expiredInput(native, input);
  expect(expired.input).toEqual(input);
  expect(await loadInput(identity)).toEqual(input);
  expect(AsyncStorage.removeItem).not.toHaveBeenCalled();
  await releaseExpiredInput(native, identity, expired);
  expect(await loadInput(identity)).toBeUndefined();
  expect(native.operation).toHaveBeenCalledWith(inputId(input));
  expect(native.submit).toHaveBeenCalledTimes(1);
  expect(groups.createRequestId).toHaveBeenCalledTimes(1);
});

test.each([protocol.GroupRetryError.Future, protocol.GroupRetryError.InvalidToken])("other retry rejection %s keeps its original failure and input", async (reason) => {
  const failure = new protocol.GroupApiError.Retry({ reason });
  const native = client(jest.fn().mockRejectedValue(failure));
  const input = captureInput(identity, intent());
  await expect(submitInput(native, identity, input, context)).rejects.toBe(failure);
  expect(await loadInput(identity)).toEqual(input);
  expect(AsyncStorage.removeItem).not.toHaveBeenCalled();
});

test.each(["read", "remove"])("failed expiry acknowledgement %s keeps the matching input fenced", async (cut) => {
  const native = client(); const input = captureInput(identity, intent());
  const expired = await expiredInput(native, input);
  if (cut === "read") jest.mocked(AsyncStorage.getItem).mockRejectedValueOnce(new Error("storage unavailable"));
  else jest.mocked(AsyncStorage.removeItem).mockRejectedValueOnce(new Error("storage unavailable"));
  await expect(releaseExpiredInput(native, identity, expired)).rejects.toThrow("storage unavailable");
  expect(await loadInput(identity)).toEqual(input);
  await releaseExpiredInput(native, identity, expired);
  expect(await loadInput(identity)).toBeUndefined();
  expect(native.submit).toHaveBeenCalledTimes(1);
});

test("expiry release refuses retained native obligations and failed evidence reads", async () => {
  const native = client(); const input = captureInput(identity, intent());
  const expired = await expiredInput(native, input);
  jest.mocked(native.operation).mockRejectedValueOnce(new Error("native unavailable"));
  await expect(releaseExpiredInput(native, identity, expired)).rejects.toThrow("native unavailable");
  const known = { ...operation(inputId(input), intent()), state: new groups.GroupOperationState.Uncertain(new protocol.GroupApiError.Unavailable()) };
  jest.mocked(native.operation).mockResolvedValue(known);
  await expect(releaseExpiredInput(native, identity, expired)).rejects.toThrow("A saved result is available");
  expect(await loadInput(identity)).toEqual(input);
  expect(AsyncStorage.removeItem).not.toHaveBeenCalled();
  expect(native.submit).toHaveBeenCalledTimes(1);
});

test("late expiry release cannot remove replacement input and cannot cross identities", async () => {
  const native = client(); const input = captureInput(identity, intent());
  const expired = await expiredInput(native, input);
  const otherIdentity = { ...identity, id: { ...identity.id, sequence: 2n } };
  await expect(releaseExpiredInput(native, otherIdentity, expired)).rejects.toThrow("different identity");
  expect(native.operation).not.toHaveBeenCalled();
  const evidence = deferred<groups.GroupOperation | undefined>();
  jest.mocked(native.operation).mockReturnValue(evidence.promise);
  const released = releaseExpiredInput(native, identity, expired);
  const rejected = expect(released).rejects.toBeInstanceOf(PendingInputConflict);
  while (jest.mocked(native.operation).mock.calls.length === 0) await Promise.resolve();
  await acknowledgeInput(input, operation(inputId(input), intent()));
  const replacement = captureInput(identity, intent("Newer"), bytes(32, 9));
  await expect(submitInput(client(jest.fn().mockRejectedValue(new Error("unknown"))), identity, replacement, context)).rejects.toThrow("unknown");
  evidence.resolve(undefined);
  await rejected;
  expect(await loadInput(identity)).toEqual(replacement);
});
