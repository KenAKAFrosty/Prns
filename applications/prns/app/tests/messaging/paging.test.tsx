import "./native-mock";
import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react-native";
import { Text, View } from "react-native";
import { usePaged } from "../../src/messaging/usePaged";
import { Paging } from "../../src/messaging/Paging";

type Page = { items: string[]; next: number | undefined };
const result = (items: string[], next?: number): Page => ({ items, next });
function deferred<T>() { let resolve!: (value: T) => void; let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
function Fixture({ owner, read, active = true, revision = 0, scope = "same-identity" }: {
  owner: object; read: (after: number | undefined, signal: AbortSignal) => Promise<Page>; active?: boolean; revision?: number; scope?: string;
}) {
  const page = usePaged({ owner, scope, active, revision, read });
  return <View><Paging label="Conversations" active={active} page={page} />{page.rows.map((row) => <Text key={row}>{row}</Text>)}</View>;
}
afterEach(cleanup);
async function press(label: string) { await act(async () => { await fireEvent.press(screen.getByRole("button", { name: label })); }); }

test("an empty filtered page follows its explicit cursor and previous preserves the range", async () => {
  const owner = {};
  const read = jest.fn(async (after: number | undefined) => after === undefined ? result([], 12) : result(["Late row"]));
  await render(<Fixture owner={owner} read={read} />);
  expect(screen.getByLabelText("Next conversations")).toBeEnabled();
  await press("Next conversations");
  expect(read.mock.calls.at(-1)?.[0]).toBe(12);
  expect(screen.getByText("Late row")).toBeTruthy();
  await press("Previous conversations");
  expect(read.mock.calls.at(-1)?.[0]).toBeUndefined();
  expect(screen.queryByText("Late row")).toBeNull();
});

test("same-tick next presses claim only one page and discard the previous rows", async () => {
  const pages = jest.spyOn(require("../../src/messaging/usePaged"), "usePaged");
  const pending = deferred<Page>();
  const read = jest.fn().mockResolvedValueOnce(result(["First"], 1)).mockReturnValueOnce(pending.promise);
  await render(<Fixture owner={{}} read={read} />);
  const next = pages.mock.results.at(-1)!.value.next;
  await act(async () => { next(); next(); });
  expect(read).toHaveBeenCalledTimes(2);
  expect(screen.queryByText("First")).toBeNull();
  await act(async () => { pending.resolve(result(["Second"], 2)); });
  expect(screen.getByLabelText("Conversations page 2")).toBeTruthy();
});

test.each(["success", "failure"])("late old-client %s cannot replace facts or unlock the new page", async (outcome) => {
  const old = deferred<Page>(), fresh = deferred<Page>();
  const first = {}, second = {};
  const readOld = jest.fn(() => old.promise), readNew = jest.fn(() => fresh.promise);
  const view = await render(<Fixture owner={first} read={readOld} />);
  await view.rerender(<Fixture owner={second} read={readNew} />);
  await act(async () => { if (outcome === "success") old.resolve(result(["Old"], 1)); else old.reject(new Error("Old failure")); });
  expect(screen.queryByText("Old")).toBeNull(); expect(screen.queryByText("Old failure")).toBeNull();
  expect(screen.queryByLabelText("Next conversations")).toBeNull();
  await act(async () => { fresh.resolve(result(["New"], 2)); });
  expect(screen.getByText("New")).toBeTruthy();
  expect(screen.getByLabelText("Next conversations")).toBeEnabled();
});

test("refresh during a later range rereads that cursor without restarting the catalog", async () => {
  const owner = {}, read = jest.fn(async (after: number | undefined) => result([after === undefined ? "First" : "Later"], after === undefined ? 50 : 100));
  const view = await render(<Fixture owner={owner} read={read} />);
  await press("Next conversations");
  await view.rerender(<Fixture owner={owner} read={read} revision={1} />);
  expect(read.mock.calls.map((call) => call[0])).toEqual([undefined, 50, 50]);
  expect(screen.getByLabelText("Conversations page 2")).toBeTruthy();
});

test("inactive pages admit no new reads and discard late results", async () => {
  const owner = {}, late = deferred<Page>(), read = jest.fn(() => late.promise);
  const view = await render(<Fixture owner={owner} read={read} />);
  await view.rerender(<Fixture owner={owner} read={read} active={false} />);
  await act(async () => { late.resolve(result(["Hidden stale"], 1)); });
  expect(screen.queryByText("Hidden stale")).toBeNull();
  expect(screen.queryByLabelText("Next conversations")).toBeNull();
  expect(read).toHaveBeenCalledTimes(1);
});


test("cursor history stays within its window while First and refresh preserve access", async () => {
  const owner = {}, read = jest.fn(async (after: number | undefined) => result([`Row ${after ?? 0}`], (after ?? 0) + 1));
  const view = await render(<Fixture owner={owner} read={read} />);
  for (let index = 0; index < 70; index++) await press("Next conversations");
  expect(screen.getByLabelText("Conversations page 71")).toBeTruthy();
  await view.rerender(<Fixture owner={owner} read={read} revision={1} />);
  expect(read.mock.calls.at(-1)?.[0]).toBe(70);
  for (let index = 0; index < 63; index++) await press("Previous conversations");
  expect(screen.getByLabelText("Conversations page 8")).toBeTruthy();
  expect(screen.getByLabelText("Previous conversations")).toBeDisabled();
  expect(screen.getByLabelText("First conversations page")).toBeEnabled();
  await press("First conversations page");
  expect(read.mock.calls.at(-1)?.[0]).toBeUndefined();
  expect(screen.getByLabelText("Conversations page 1")).toBeTruthy();
  expect(screen.getByText("Row 0")).toBeTruthy();
});

test.each(["owner", "scope"])("%s replacement never admits the old cursor into the new scope", async (replacement) => {
  const first = {}, second = {};
  const read = jest.fn(async (after: number | undefined) => result([`Row ${after ?? 0}`], (after ?? 0) + 1));
  const view = await render(<Fixture owner={first} read={read} />);
  await press("Next conversations");
  read.mockClear();
  await view.rerender(<Fixture owner={replacement === "owner" ? second : first} scope={replacement === "scope" ? "new-control" : "same-identity"} read={read} />);
  expect(read.mock.calls.map((call) => call[0])).toEqual([undefined]);
});

test("First invalidates an old completed page's pending refresh", async () => {
  const owner = {}, late = deferred<Page>();
  const read = jest.fn().mockResolvedValueOnce(result(["First"], 1))
    .mockResolvedValueOnce(result(["Second"], 2)).mockReturnValueOnce(late.promise)
    .mockResolvedValueOnce(result(["Fresh second"], 2)).mockResolvedValueOnce(result(["Fresh first"], 1));
  const view = await render(<Fixture owner={owner} read={read} />);
  await press("Next conversations");
  await view.rerender(<Fixture owner={owner} read={read} revision={1} />);
  // A hidden page aborts its refresh; First is available when it becomes active again.
  await view.rerender(<Fixture owner={owner} read={read} active={false} revision={1} />);
  await view.rerender(<Fixture owner={owner} read={read} revision={1} />);
  await press("First conversations page");
  await act(async () => { late.resolve(result(["Obsolete second"], 9)); });
  expect(screen.queryByText("Obsolete second")).toBeNull();
  expect(screen.getByText("Fresh first")).toBeTruthy();
  expect(screen.getByLabelText("Conversations page 1")).toBeTruthy();
});


test("navigation during a background read aborts it and accepts only the chosen page", async () => {
  const owner = {}, refresh = deferred<Page>(), nextPage = deferred<Page>();
  const read = jest.fn<Promise<Page>, [number | undefined, AbortSignal]>()
    .mockResolvedValueOnce(result(["First"], 1)).mockReturnValueOnce(refresh.promise).mockReturnValueOnce(nextPage.promise);
  const hook = await renderHook(({ revision }: { revision: number }) => usePaged({ owner, scope: "catalog", active: true, revision, read }), {
    initialProps: { revision: 0 },
  });
  await waitFor(() => expect(hook.result.current.loaded).toBe(true));
  await act(async () => { await hook.rerender({ revision: 1 }); });
  expect(hook.result.current.busy).toBe(true);
  expect(hook.result.current.loaded).toBe(true);
  await act(async () => { hook.result.current.next(); hook.result.current.next(); });
  expect(read).toHaveBeenCalledTimes(3);
  expect(read.mock.calls[1]![1].aborted).toBe(true);
  expect(read.mock.calls[2]![0]).toBe(1);
  expect(hook.result.current.rows).toEqual([]);
  await act(async () => { nextPage.resolve(result(["Second"], 2)); });
  expect(hook.result.current.page).toBe(2);
  expect(hook.result.current.rows).toEqual(["Second"]);
  await act(async () => { refresh.resolve(result(["Obsolete first"], 9)); });
  expect(hook.result.current.page).toBe(2);
  expect(hook.result.current.rows).toEqual(["Second"]);
});

afterEach(() => { jest.restoreAllMocks(); });
