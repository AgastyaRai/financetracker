import { StrictMode } from "react";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import IndexingProgress from "./IndexingProgress";

const api = vi.hoisted(() => ({ getIndexingStatus: vi.fn() }));
vi.mock("../api", () => api);

type Counts = { total: number; indexed: number; remaining: number };
const pending: Counts = { total: 331, indexed: 172, remaining: 159 };
const complete: Counts = { total: 331, indexed: 331, remaining: 0 };
const preparing = "Preparing search: 172 of 331 transactions ready, 159 remaining.";
const ready = "All 331 transactions ready for search";

function deferred() {
  let resolve!: (value: Counts) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<Counts>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

async function mount() {
  const view = render(<IndexingProgress />);
  await act(async () => {});
  return view;
}

async function advance(milliseconds = 5_000) {
  await act(async () => { await vi.advanceTimersByTimeAsync(milliseconds); });
}

beforeEach(() => {
  vi.useFakeTimers();
  api.getIndexingStatus.mockReset().mockResolvedValue(pending);
  localStorage.setItem("access_token", "current-session");
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("indexing progress", () => {
  it("shows loading without claiming completion, then reads fresh counts after a reload", async () => {
    // Hold the first response so loading cannot accidentally look like an empty or finished account.
    const first = deferred();
    api.getIndexingStatus.mockReturnValueOnce(first.promise);
    const view = await mount();
    expect(screen.getByText("Loading search indexing progress…")).toBeInTheDocument();
    expect(screen.queryByText(/ready for search|No transactions to index/)).not.toBeInTheDocument();
    await act(async () => first.resolve(pending));
    expect(screen.getByText(preparing)).toBeInTheDocument();

    // A reload must ask the server again instead of reusing the previous component's counts.
    view.unmount();
    api.getIndexingStatus.mockResolvedValue(complete);
    await mount();
    expect(screen.getByText(ready)).toBeInTheDocument();
    expect(api.getIndexingStatus).toHaveBeenCalledTimes(2);
  });

  it("updates every five seconds after completion of a request and stops when all rows are ready", async () => {
    api.getIndexingStatus.mockResolvedValueOnce(pending)
      .mockResolvedValueOnce({ total: 331, indexed: 300, remaining: 31 }).mockResolvedValue(complete);
    await mount();
    expect(screen.getByText(preparing)).toBeInTheDocument();
    await advance(4_999);
    expect(api.getIndexingStatus).toHaveBeenCalledTimes(1);
    await advance(1);
    expect(screen.getByText("Preparing search: 300 of 331 transactions ready, 31 remaining.")).toBeInTheDocument();
    await advance();
    expect(screen.getByText(ready)).toBeInTheDocument();
    await advance(30_000);
    expect(api.getIndexingStatus).toHaveBeenCalledTimes(3);
  });

  it("shows an empty account and does not keep polling it", async () => {
    api.getIndexingStatus.mockResolvedValue({ total: 0, indexed: 0, remaining: 0 });
    await mount();
    expect(screen.getByText("No transactions to index")).toBeInTheDocument();
    await advance(30_000);
    expect(api.getIndexingStatus).toHaveBeenCalledOnce();
  });

  it("waits for a slow poll to finish before starting the next five-second delay", async () => {
    const slow = deferred();
    api.getIndexingStatus.mockResolvedValueOnce(pending).mockReturnValueOnce(slow.promise).mockResolvedValue(complete);
    await mount();
    expect(screen.getByText(preparing)).toBeInTheDocument();
    await advance();
    await advance(20_000);
    expect(api.getIndexingStatus).toHaveBeenCalledTimes(2);

    // Finishing this response starts a new delay; elapsed time while it was pending does not count.
    await act(async () => slow.resolve(pending));
    await advance(4_999);
    expect(api.getIndexingStatus).toHaveBeenCalledTimes(2);
    await advance(1);
    expect(screen.getByText(ready)).toBeInTheDocument();
    expect(api.getIndexingStatus).toHaveBeenCalledTimes(3);
  });

  it.each(["initial read", "later poll"])("pauses after a failed %s and resumes only after Retry", async (stage) => {
    if (stage === "later poll") api.getIndexingStatus.mockResolvedValueOnce(pending);
    api.getIndexingStatus.mockRejectedValueOnce(new Error("status unavailable")).mockResolvedValue(pending);
    await mount();
    if (stage === "later poll") {
      expect(screen.getByText(preparing)).toBeInTheDocument();
      await advance();
    }
    expect(screen.getByText("Could not load search indexing progress.")).toBeInTheDocument();
    expect(screen.queryByText(preparing)).not.toBeInTheDocument();
    expect(screen.queryByText(ready)).not.toBeInTheDocument();
    const failedCalls = api.getIndexingStatus.mock.calls.length;
    await advance(30_000);
    expect(api.getIndexingStatus).toHaveBeenCalledTimes(failedCalls);

    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Retry indexing progress" })));
    expect(screen.getByText(preparing)).toBeInTheDocument();
    await advance();
    expect(api.getIndexingStatus).toHaveBeenCalledTimes(failedCalls + 2);
  });

  it.each(["success", "failure"])("aborts on unmount and ignores a late %s", async (outcome) => {
    const old = deferred();
    api.getIndexingStatus.mockReturnValueOnce(old.promise);
    const view = await mount();
    expect(api.getIndexingStatus).toHaveBeenCalledOnce();
    const signal = api.getIndexingStatus.mock.calls[0][0] as AbortSignal;
    view.unmount();
    expect(signal.aborted).toBe(true);
    await act(async () => outcome === "success" ? old.resolve(pending) : old.reject(new Error("late failure")));
    await advance(30_000);
    expect(api.getIndexingStatus).toHaveBeenCalledOnce();
    expect(screen.queryByText(/Preparing search|Could not load/)).not.toBeInTheDocument();
  });

  it("clears a scheduled poll on unmount", async () => {
    const view = await mount();
    expect(screen.getByText(preparing)).toBeInTheDocument();
    view.unmount();
    await advance(30_000);
    expect(api.getIndexingStatus).toHaveBeenCalledOnce();
  });

  it("does not apply or repeat a response after the stored session changes", async () => {
    const old = deferred();
    api.getIndexingStatus.mockReturnValueOnce(old.promise);
    await mount();
    expect(api.getIndexingStatus).toHaveBeenCalledOnce();
    localStorage.setItem("access_token", "replacement-session");
    await act(async () => old.resolve(pending));
    await advance(30_000);
    expect(api.getIndexingStatus).toHaveBeenCalledOnce();
    expect(screen.queryByText(preparing)).not.toBeInTheDocument();
  });

  it("keeps one polling chain after StrictMode cancels its first setup", async () => {
    const cancelled = deferred();
    api.getIndexingStatus.mockReturnValueOnce(cancelled.promise).mockResolvedValue(pending);
    render(<StrictMode><IndexingProgress /></StrictMode>);
    await act(async () => {});
    expect(screen.getByText(preparing)).toBeInTheDocument();
    expect(api.getIndexingStatus).toHaveBeenCalledTimes(2);
    expect((api.getIndexingStatus.mock.calls[0][0] as AbortSignal).aborted).toBe(true);

    // Even a provider mock that ignores abort must not revive the cancelled setup's timer.
    await act(async () => cancelled.resolve(complete));
    expect(screen.queryByText(ready)).not.toBeInTheDocument();
    await advance();
    expect(api.getIndexingStatus).toHaveBeenCalledTimes(3);
    await advance();
    expect(api.getIndexingStatus).toHaveBeenCalledTimes(4);
  });
});
