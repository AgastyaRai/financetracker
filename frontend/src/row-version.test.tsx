import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";

function monthValue() {
  const date = new Date();
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}`;
}

function originalTransaction() {
  return {
    id: "row-version-coffee", amount: "-10.00", kind: "Expense", category: "Food",
    date: monthValue() + "-01", description: "Imported coffee", can_revert: false, row_version: 1,
  };
}

function jsonResponse(value: unknown, status = 200) {
  return new Response(JSON.stringify(value), { status, headers: { "Content-Type": "application/json" } });
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((complete) => { resolve = complete; });
  return { promise, resolve };
}

let savedTransaction = originalTransaction();
let updateBodies: Array<Record<string, unknown>> = [];

beforeEach(() => {
  savedTransaction = originalTransaction();
  updateBodies = [];
  const payload = btoa(JSON.stringify({ exp: Math.floor(Date.now() / 1000) + 3600 }));
  localStorage.setItem("access_token", `header.${payload}.signature`);

  // Exercise the real API client; all HTTP work stays inside this synthetic server contract.
  vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    const method = init?.method ?? "GET";
    if (method === "GET" && url === "/api/transactions") return jsonResponse([savedTransaction]);
    if (method === "GET" && url === "/api/transactions/indexing-status") {
      return jsonResponse({ total: 1, indexed: 1, remaining: 0 });
    }
    if (method === "GET" && url === "/api/bank-connections") {
      return jsonResponse([{ connection_id: "row-version-bank", provider: "plaid", status: "active" }]);
    }
    if (method === "GET" && url.startsWith("/api/budgets")) return jsonResponse([]);
    if (method === "POST" && url === "/api/bank-connections/row-version-bank/import") {
      savedTransaction = { ...savedTransaction, amount: "-12.00", date: monthValue() + "-02", row_version: 2 };
      return jsonResponse({ imported: 0 });
    }
    if (method === "PUT" && url === "/api/transactions/row-version-coffee") {
      const body = JSON.parse(String(init?.body)) as Record<string, unknown>;
      updateBodies.push(body);
      if (body.expected_version !== savedTransaction.row_version) {
        return jsonResponse({ error: "Transaction changed while you were editing. Reload the latest values before saving." }, 409);
      }
      savedTransaction = { ...savedTransaction, ...body, row_version: savedTransaction.row_version + 1 };
      return jsonResponse({ row_version: savedTransaction.row_version });
    }
    throw new Error(`Unexpected network request in row-version test: ${method} ${url}`);
  }));
});

afterEach(() => {
  vi.unstubAllGlobals();
});

async function openEditor(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("button", { name: /Edit transaction: Imported coffee/ }));
  return within(screen.getByRole("form", { name: "Edit transaction" }).closest("tr") as HTMLElement);
}

describe("transaction draft versions", () => {
  it("sends the draft's original version even after an import refreshes the list", async () => {
    const user = userEvent.setup();
    render(<App />);
    const editor = await openEditor(user);
    await user.clear(editor.getByLabelText("Description (optional)"));
    await user.type(editor.getByLabelText("Description (optional)"), "My draft description");

    await user.click(await screen.findByRole("button", { name: "Import again" }));
    const summary = screen.getByRole("heading", { name: "Summary" }).closest(".card") as HTMLElement;
    await waitFor(() => expect(within(summary).getByText("Money out").closest(".metricTile")).toHaveTextContent("$12.00"));
    expect(editor.getByLabelText("Amount")).toHaveValue(-10);
    await user.click(editor.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(updateBodies).toHaveLength(1));
    expect(updateBodies[0]).toEqual(expect.objectContaining({
      amount: "-10.00", date: monthValue() + "-01", description: "My draft description", expected_version: 1,
    }));
    expect(savedTransaction.amount).toBe("-12.00");
  });

  it("preserves a conflicting draft until the user explicitly reloads and reviews the latest transaction", async () => {
    const user = userEvent.setup();
    render(<App />);
    const editor = await openEditor(user);
    await user.clear(editor.getByLabelText("Description (optional)"));
    await user.type(editor.getByLabelText("Description (optional)"), "My unsaved note");

    // Another tab/backend completes an import without refreshing this tab's draft.
    savedTransaction = { ...savedTransaction, amount: "-12.00", date: monthValue() + "-02", row_version: 2 };
    await user.click(editor.getByRole("button", { name: "Save changes" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Transaction changed while you were editing");
    expect(editor.getByLabelText("Description (optional)")).toHaveValue("My unsaved note");
    expect(editor.getByLabelText("Amount")).toHaveValue(-10);
    expect(screen.queryByText("Transaction updated successfully.")).not.toBeInTheDocument();
    expect(updateBodies).toHaveLength(1);
    expect(savedTransaction.amount).toBe("-12.00");

    await user.click(screen.getByRole("button", { name: /reload latest/i }));
    const reloaded = within(screen.getByRole("form", { name: "Edit transaction" }).closest("tr") as HTMLElement);
    await waitFor(() => expect(reloaded.getByLabelText("Amount")).toHaveValue(-12));
    expect(reloaded.getByLabelText("Date")).toHaveValue(monthValue() + "-02");
    expect(reloaded.getByLabelText("Description (optional)")).toHaveValue("Imported coffee");
    expect(updateBodies).toHaveLength(1);

    await user.clear(reloaded.getByLabelText("Description (optional)"));
    await user.type(reloaded.getByLabelText("Description (optional)"), "Reviewed note");
    await user.click(reloaded.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(updateBodies).toHaveLength(2));
    expect(updateBodies[1]).toEqual(expect.objectContaining({
      amount: "-12.00", date: monthValue() + "-02", description: "Reviewed note", expected_version: 2,
    }));
    expect(await screen.findByText("Transaction updated successfully.")).toBeInTheDocument();
  });
  it.each(["import-first", "reload-first"])("completes an explicit reload superseded by an import refresh (%s)", async (responseOrder) => {
    const user = userEvent.setup();
    render(<App />);
    const editor = await openEditor(user);
    await user.clear(editor.getByLabelText("Description (optional)"));
    await user.type(editor.getByLabelText("Description (optional)"), "Unsaved note");
    savedTransaction = { ...savedTransaction, amount: "-12.00", date: monthValue() + "-02", row_version: 2 };
    await user.click(editor.getByRole("button", { name: "Save changes" }));
    await screen.findByRole("alert");

    const importReply = deferred<Response>();
    const reloadReply = deferred<Response>();
    const dashboardReply = deferred<Response>();
    const reloadSnapshot = { ...savedTransaction };
    const initialFetch = vi.mocked(fetch).getMockImplementation()!;
    let transactionReads = 0;
    vi.mocked(fetch).mockImplementation(async (input, init) => {
      if (String(input) === "/api/bank-connections/row-version-bank/import") return importReply.promise;
      if (String(input) === "/api/transactions" && init?.method === "GET") {
        transactionReads++;
        if (transactionReads === 1) return reloadReply.promise;
        if (transactionReads === 2) return dashboardReply.promise;
      }
      return initialFetch(input, init);
    });

    try {
      await user.click(screen.getByRole("button", { name: "Import again" }));
      await user.click(screen.getByRole("button", { name: "Reload latest" }));
      await waitFor(() => expect(transactionReads).toBe(1));
      expect(screen.getByRole("button", { name: "Reloading…" })).toBeDisabled();
      expect(editor.getByLabelText("Description (optional)")).toHaveValue("Unsaved note");

      savedTransaction = { ...savedTransaction, amount: "-13.00", date: monthValue() + "-03", row_version: 3 };
      await act(async () => importReply.resolve(jsonResponse({ imported: 0 })));
      await waitFor(() => expect(transactionReads).toBe(2));
      if (responseOrder === "import-first") {
        await act(async () => dashboardReply.resolve(jsonResponse([savedTransaction])));
        await act(async () => reloadReply.resolve(jsonResponse([reloadSnapshot])));
      } else {
        await act(async () => reloadReply.resolve(jsonResponse([reloadSnapshot])));
        await act(async () => dashboardReply.resolve(jsonResponse([savedTransaction])));
      }

      await waitFor(() => expect(within(screen.getByRole("form", { name: "Edit transaction" }).closest("tr") as HTMLElement).getByLabelText("Amount")).toHaveValue(-13));
      const reloaded = within(screen.getByRole("form", { name: "Edit transaction" }).closest("tr") as HTMLElement);
      expect(reloaded.getByLabelText("Date")).toHaveValue(monthValue() + "-03");
      expect(reloaded.getByLabelText("Description (optional)")).toHaveValue("Imported coffee");
      expect(reloaded.getByRole("button", { name: "Save changes" })).toBeEnabled();
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
      expect(updateBodies).toHaveLength(1);

      await user.clear(reloaded.getByLabelText("Description (optional)"));
      await user.type(reloaded.getByLabelText("Description (optional)"), "Reviewed latest import");
      await user.click(reloaded.getByRole("button", { name: "Save changes" }));
      await screen.findByText("Transaction updated successfully.");
      expect(updateBodies).toHaveLength(2);
      expect(updateBodies[1]).toEqual(expect.objectContaining({
        amount: "-13.00", date: monthValue() + "-03", expected_version: 3, description: "Reviewed latest import",
      }));
    } finally {
      await act(async () => {
        importReply.resolve(jsonResponse({ imported: 0 }));
        reloadReply.resolve(jsonResponse([reloadSnapshot]));
        dashboardReply.resolve(jsonResponse([savedTransaction]));
      });
    }
  });

});

async function openConflictingDraft(user: ReturnType<typeof userEvent.setup>) {
  render(<App />);
  const editor = await openEditor(user);
  await user.clear(editor.getByLabelText("Description (optional)"));
  await user.type(editor.getByLabelText("Description (optional)"), "Unsaved note");
  savedTransaction = { ...savedTransaction, amount: "-12.00", date: monthValue() + "-02", row_version: 2 };
  await user.click(editor.getByRole("button", { name: "Save changes" }));
  await screen.findByRole("alert");
}

function holdReload(outcome: "success" | "http-error" | "network-error" | "removed" | "pending" = "success") {
  const oldReply = deferred<Response>();
  const latestReply = deferred<Response>();
  const oldSnapshot = { ...savedTransaction };
  const initialFetch = vi.mocked(fetch).getMockImplementation()!;
  let transactionReads = 0;
  let oldSettled = false;
  void oldReply.promise.then(() => { oldSettled = true; });
  vi.mocked(fetch).mockImplementation(async (input, init) => {
    if (String(input) === "/api/bank-connections/row-version-bank/import") {
      savedTransaction = { ...savedTransaction, amount: "-13.00", date: monthValue() + "-03", row_version: 3 };
      return jsonResponse({ imported: 0 });
    }
    if (String(input) === "/api/transactions" && init?.method === "GET") {
      transactionReads++;
      if (transactionReads === 1) return oldReply.promise;
      if (transactionReads === 2) {
        if (outcome === "http-error") return jsonResponse({ error: "Latest refresh failed" }, 500);
        if (outcome === "network-error") throw new TypeError("Latest refresh failed");
        if (outcome === "removed") return jsonResponse([]);
        if (outcome === "pending") return latestReply.promise;
      }
    }
    return initialFetch(input, init);
  });
  return {
    oldReply, latestReply, oldSnapshot,
    reads: () => transactionReads,
    oldSettled: () => oldSettled,
    settle: async () => act(async () => {
      oldReply.resolve(jsonResponse([oldSnapshot]));
      latestReply.resolve(jsonResponse([savedTransaction]));
    }),
  };
}

describe("reload recovery while an obsolete read is still pending", () => {
  it.each([200, 500])("recovers and saves before the obsolete read settles, ignoring its later %s response", async (oldStatus) => {
    const user = userEvent.setup();
    await openConflictingDraft(user);
    const held = holdReload();
    try {
      await user.click(screen.getByRole("button", { name: "Reload latest" }));
      await waitFor(() => expect(held.reads()).toBe(1));
      await user.click(screen.getByRole("button", { name: "Import again" }));
      await waitFor(() => expect(held.reads()).toBe(2));
      const summary = screen.getByRole("heading", { name: "Summary" }).closest(".card") as HTMLElement;
      await waitFor(() => expect(within(summary).getByText("Money out").closest(".metricTile")).toHaveTextContent("$13.00"));

      // The obsolete response remains unresolved through recovery and the reviewed save.
      await waitFor(() => expect(within(screen.getByRole("form", { name: "Edit transaction" }).closest("tr") as HTMLElement).getByLabelText("Amount")).toHaveValue(-13));
      expect(held.oldSettled()).toBe(false);
      const editor = within(screen.getByRole("form", { name: "Edit transaction" }).closest("tr") as HTMLElement);
      expect(editor.getByLabelText("Date")).toHaveValue(monthValue() + "-03");
      expect(editor.getByRole("button", { name: "Save changes" })).toBeEnabled();
      expect(editor.getByRole("button", { name: "Cancel" })).toBeEnabled();
      expect(updateBodies).toHaveLength(1);
      await user.clear(editor.getByLabelText("Description (optional)"));
      await user.type(editor.getByLabelText("Description (optional)"), "Reviewed current values");
      await user.click(editor.getByRole("button", { name: "Save changes" }));
      await screen.findByText("Transaction updated successfully.");
      expect(held.oldSettled()).toBe(false);
      expect(updateBodies[1]).toEqual(expect.objectContaining({ amount: "-13.00", date: monthValue() + "-03", expected_version: 3 }));

      await act(async () => held.oldReply.resolve(jsonResponse(oldStatus === 200 ? [held.oldSnapshot] : { error: "Obsolete read failed" }, oldStatus)));
      expect(screen.queryByRole("form", { name: "Edit transaction" })).not.toBeInTheDocument();
      expect(screen.queryByText("Obsolete read failed")).not.toBeInTheDocument();
      expect(screen.getByText("Reviewed current values")).toBeInTheDocument();
    } finally { await held.settle(); }
  });

  it.each(["http-error", "network-error"] as const)("reports the current %s without waiting for the obsolete read and allows an explicit retry", async (failure) => {
    const user = userEvent.setup();
    await openConflictingDraft(user);
    const held = holdReload(failure);
    try {
      await user.click(screen.getByRole("button", { name: "Reload latest" }));
      await waitFor(() => expect(held.reads()).toBe(1));
      await user.click(screen.getByRole("button", { name: "Import again" }));
      await screen.findByText("Latest refresh failed");
      expect(held.oldSettled()).toBe(false);
      expect(held.reads()).toBe(2);
      const editor = within(screen.getByRole("form", { name: "Edit transaction" }).closest("tr") as HTMLElement);
      expect(editor.getByLabelText("Amount")).toHaveValue(-10);
      expect(editor.getByLabelText("Description (optional)")).toHaveValue("Unsaved note");
      expect(editor.getByRole("button", { name: "Save changes" })).toBeDisabled();
      expect(screen.getByRole("button", { name: "Reload latest" })).toBeEnabled();
      await user.click(screen.getByRole("button", { name: "Reload latest" }));
      await waitFor(() => expect(within(screen.getByRole("form", { name: "Edit transaction" }).closest("tr") as HTMLElement).getByLabelText("Amount")).toHaveValue(-13));
      expect(held.oldSettled()).toBe(false);
      expect(updateBodies).toHaveLength(1);
    } finally { await held.settle(); }
  });

  it("closes an unavailable editor before the obsolete read settles", async () => {
    const user = userEvent.setup();
    await openConflictingDraft(user);
    const held = holdReload("removed");
    try {
      await user.click(screen.getByRole("button", { name: "Reload latest" }));
      await waitFor(() => expect(held.reads()).toBe(1));
      await user.click(screen.getByRole("button", { name: "Import again" }));
      await screen.findByText("Transaction is no longer available.");
      expect(held.oldSettled()).toBe(false);
      expect(screen.queryByRole("form", { name: "Edit transaction" })).not.toBeInTheDocument();
      expect(updateBodies).toHaveLength(1);
    } finally { await held.settle(); }
  });

  it.each([200, 500, 401])("ignores old pending reads with a late %s after logout and a new session's pending save", async (oldStatus) => {
    const user = userEvent.setup();
    await openConflictingDraft(user);
    const held = holdReload("pending");
    const writeReply = deferred<Response>();
    try {
      await user.click(screen.getByRole("button", { name: "Reload latest" }));
      await waitFor(() => expect(held.reads()).toBe(1));
      await user.click(screen.getByRole("button", { name: "Import again" }));
      await waitFor(() => expect(held.reads()).toBe(2));
      await user.click(screen.getByRole("button", { name: "Logout" }));
      expect(localStorage.getItem("access_token")).toBeNull();
      const nextToken = `header.${btoa(JSON.stringify({ exp: Math.floor(Date.now() / 1000) + 7200 }))}.signature`;
      let nextTransaction = { ...originalTransaction(), id: "next-coffee", amount: "-7.00", description: "Next user coffee" };
      const previousFetch = vi.mocked(fetch).getMockImplementation()!;
      let nextSession = false;
      let nextSave: Record<string, unknown> | undefined;
      vi.mocked(fetch).mockImplementation(async (input, init) => {
        if (String(input) === "/api/users/login") {
          nextSession = true;
          return jsonResponse({ access_token: nextToken, user_id: "next-user" });
        }
        if (nextSession && String(input) === "/api/transactions" && init?.method === "GET") return jsonResponse([nextTransaction]);
        if (String(input) === "/api/transactions/next-coffee" && init?.method === "PUT") {
          nextSave = JSON.parse(String(init.body)) as Record<string, unknown>;
          nextTransaction = { ...nextTransaction, ...nextSave, row_version: nextTransaction.row_version + 1 };
          return writeReply.promise;
        }
        return previousFetch(input, init);
      });
      await user.type(screen.getByLabelText("Username or Email"), "next-user");
      await user.type(screen.getByLabelText("Password"), "password{Enter}");
      await user.click(await screen.findByRole("button", { name: /Edit transaction: Next user coffee/ }));
      const editor = within(screen.getByRole("form", { name: "Edit transaction" }).closest("tr") as HTMLElement);
      await user.clear(editor.getByLabelText("Description (optional)"));
      await user.type(editor.getByLabelText("Description (optional)"), "Next draft");
      await user.click(editor.getByRole("button", { name: "Save changes" }));
      await waitFor(() => expect(nextSave).toBeDefined());
      const readsBeforeOldReplies = vi.mocked(fetch).mock.calls.filter(([input, init]) => String(input) === "/api/transactions" && init?.method === "GET").length;
      await act(async () => {
        held.oldReply.resolve(jsonResponse([held.oldSnapshot]));
        held.latestReply.resolve(jsonResponse(oldStatus === 200 ? [savedTransaction] : { error: "Old session failed" }, oldStatus));
      });
      expect(localStorage.getItem("access_token")).toBe(nextToken);
      expect(editor.getByLabelText("Description (optional)")).toHaveValue("Next draft");
      expect(editor.getByRole("button", { name: "Saving…" })).toBeDisabled();
      expect(screen.queryByText("Old session failed")).not.toBeInTheDocument();
      expect(vi.mocked(fetch).mock.calls.filter(([input, init]) => String(input) === "/api/transactions" && init?.method === "GET")).toHaveLength(readsBeforeOldReplies);
      await act(async () => writeReply.resolve(new Response(null, { status: 200 })));
      await screen.findByText("Transaction updated successfully.");
    } finally {
      await held.settle();
      await act(async () => writeReply.resolve(new Response(null, { status: 200 })));
    }
  });

  it.each(["http-error", "network-error"] as const)("preserves the draft after a current reload %s and does not automatically retry", async (failure) => {
    const user = userEvent.setup();
    await openConflictingDraft(user);
    const initialFetch = vi.mocked(fetch).getMockImplementation()!;
    let reads = 0;
    vi.mocked(fetch).mockImplementation(async (input, init) => {
      if (String(input) === "/api/transactions" && init?.method === "GET" && ++reads === 1) {
        if (failure === "network-error") throw new TypeError("Current reload failed");
        return jsonResponse({ error: "Current reload failed" }, 500);
      }
      return initialFetch(input, init);
    });
    await user.click(screen.getByRole("button", { name: "Reload latest" }));
    await screen.findByText("Current reload failed");
    expect(reads).toBe(1);
    const editor = within(screen.getByRole("form", { name: "Edit transaction" }).closest("tr") as HTMLElement);
    expect(editor.getByLabelText("Description (optional)")).toHaveValue("Unsaved note");
    expect(editor.getByRole("button", { name: "Save changes" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Reload latest" })).toBeEnabled();
    await user.click(screen.getByRole("button", { name: "Reload latest" }));
    await waitFor(() => expect(within(screen.getByRole("form", { name: "Edit transaction" }).closest("tr") as HTMLElement).getByLabelText("Amount")).toHaveValue(-12));
  });

  it("logs out on a current reload 401 without retrying or restoring the editor", async () => {
    const user = userEvent.setup();
    await openConflictingDraft(user);
    const initialFetch = vi.mocked(fetch).getMockImplementation()!;
    let reads = 0;
    vi.mocked(fetch).mockImplementation(async (input, init) => {
      if (String(input) === "/api/transactions" && init?.method === "GET") {
        reads++;
        return jsonResponse({ error: "Expired" }, 401);
      }
      return initialFetch(input, init);
    });
    await user.click(screen.getByRole("button", { name: "Reload latest" }));
    await screen.findByLabelText("Username or Email");
    expect(localStorage.getItem("access_token")).toBeNull();
    expect(reads).toBe(1);
    expect(screen.queryByRole("form", { name: "Edit transaction" })).not.toBeInTheDocument();
  });
});
