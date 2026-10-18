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
    id: "row-version-coffee",
    amount: "-10.00",
    kind: "Expense",
    category: "Food",
    date: monthValue() + "-01",
    description: "Imported coffee",
    can_revert: false,
    row_version: 1,
  };
}

function jsonResponse(value: unknown, status = 200) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

let savedTransaction = originalTransaction();
let updateBodies: Array<Record<string, unknown>> = [];

beforeEach(() => {
  // start each test with the original saved transaction and a valid login.
  savedTransaction = originalTransaction();
  updateBodies = [];
  const payload = btoa(JSON.stringify({ exp: Math.floor(Date.now() / 1000) + 3600 }));
  localStorage.setItem("access_token", `header.${payload}.signature`);

  // use the real API client with a mocked fetch so no request leaves the test.
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
      savedTransaction = {
        ...savedTransaction,
        amount: "-12.00",
        date: monthValue() + "-02",
        row_version: 2,
      };
      return jsonResponse({ imported: 0 });
    }
    if (method === "PUT" && url === "/api/transactions/row-version-coffee") {
      const body = JSON.parse(String(init?.body)) as Record<string, unknown>;
      updateBodies.push(body);
      if (body.expected_version !== savedTransaction.row_version) {
        return jsonResponse(
          { error: "Transaction changed while you were editing. Reload the latest values before saving." },
          409,
        );
      }
      savedTransaction = {
        ...savedTransaction,
        ...body,
        row_version: savedTransaction.row_version + 1,
      };
      return new Response(null, { status: 200 });
    }
    throw new Error(`Unexpected network request in row-version test: ${method} ${url}`);
  }));
});

afterEach(() => {
  vi.unstubAllGlobals();
});

async function openEditor(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("button", { name: /Edit transaction: Imported coffee/ }));
  const editorForm = screen.getByRole("form", { name: "Edit transaction" });
  const editorRow = editorForm.closest("tr") as HTMLElement;
  return within(editorRow);
}

describe("transaction draft versions", () => {
  it.each(["http-error", "network-error"])("keeps the saved version for a second edit after its refresh fails (%s)", async (refreshFailure) => {
    const user = userEvent.setup();
    render(<App />);
    const editor = await openEditor(user);

    // fail only the read after the first successful save; the next edit must still use its saved version.
    const initialFetch = vi.mocked(fetch).getMockImplementation()!;
    let refreshFailed = false;
    vi.mocked(fetch).mockImplementation(async (input, init) => {
      if (String(input) === "/api/transactions" && init?.method === "GET" && updateBodies.length > 0 && !refreshFailed) {
        refreshFailed = true;
        if (refreshFailure === "network-error") throw new TypeError("Refresh unavailable");
        return jsonResponse({ error: "Refresh unavailable" }, 500);
      }
      return initialFetch(input, init);
    });
    await user.clear(editor.getByLabelText("Description (optional)"));
    await user.type(editor.getByLabelText("Description (optional)"), "First saved note");
    await user.click(editor.getByRole("button", { name: "Save changes" }));
    const refreshWarning = await screen.findByRole("alert");
    expect(refreshWarning).toHaveTextContent("Transaction was updated, but the dashboard could not refresh: Refresh unavailable");
    expect(savedTransaction.row_version).toBe(2);
    expect(savedTransaction.description).toBe("First saved note");

    // reopen the locally retained row without another writer, then save a second description.
    await user.click(screen.getByRole("button", { name: /Edit transaction: First saved note/ }));
    const secondForm = screen.getByRole("form", { name: "Edit transaction" });
    const secondRow = secondForm.closest("tr") as HTMLElement;
    const secondEditor = within(secondRow);
    await user.clear(secondEditor.getByLabelText("Description (optional)"));
    await user.type(secondEditor.getByLabelText("Description (optional)"), "Second saved note");
    await user.click(secondEditor.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(updateBodies).toHaveLength(2));
    expect(updateBodies[0].expected_version).toBe(1);
    expect(updateBodies[1].expected_version).toBe(2);
    expect(await screen.findByText("Transaction updated successfully.")).toBeInTheDocument();
    expect(savedTransaction.description).toBe("Second saved note");
    expect(savedTransaction.row_version).toBe(3);
    expect(savedTransaction.amount).toBe("-10.00");
    expect(savedTransaction.date).toBe(monthValue() + "-01");
    expect(screen.queryByRole("form", { name: "Edit transaction" })).not.toBeInTheDocument();
  });

  it("still rejects a later import when a second edit follows a failed refresh", async () => {
    const user = userEvent.setup();
    render(<App />);
    const editor = await openEditor(user);
    const initialFetch = vi.mocked(fetch).getMockImplementation()!;
    let refreshFailed = false;
    vi.mocked(fetch).mockImplementation(async (input, init) => {
      if (String(input) === "/api/transactions" && init?.method === "GET" && updateBodies.length > 0 && !refreshFailed) {
        refreshFailed = true;
        return jsonResponse({ error: "Refresh unavailable" }, 500);
      }
      return initialFetch(input, init);
    });
    await user.clear(editor.getByLabelText("Description (optional)"));
    await user.type(editor.getByLabelText("Description (optional)"), "First saved note");
    await user.click(editor.getByRole("button", { name: "Save changes" }));
    const refreshWarning = await screen.findByRole("alert");
    expect(refreshWarning).toHaveTextContent("dashboard could not refresh");

    // an import in another tab advances the stored row without refreshing this tab's saved snapshot.
    savedTransaction = {
      ...savedTransaction,
      amount: "-12.00",
      date: monthValue() + "-02",
      row_version: 3,
    };
    await user.click(screen.getByRole("button", { name: /Edit transaction: First saved note/ }));
    const secondForm = screen.getByRole("form", { name: "Edit transaction" });
    const secondRow = secondForm.closest("tr") as HTMLElement;
    const secondEditor = within(secondRow);
    await user.clear(secondEditor.getByLabelText("Description (optional)"));
    await user.type(secondEditor.getByLabelText("Description (optional)"), "New unsaved note");
    await user.click(secondEditor.getByRole("button", { name: "Save changes" }));
    const conflict = await screen.findByRole("alert");
    expect(conflict).toHaveTextContent("Transaction changed while you were editing");
    expect(updateBodies).toHaveLength(2);
    expect(updateBodies[1].expected_version).toBe(2);
    expect(savedTransaction.amount).toBe("-12.00");
    expect(savedTransaction.date).toBe(monthValue() + "-02");
    expect(savedTransaction.description).toBe("First saved note");
    expect(savedTransaction.row_version).toBe(3);
    expect(secondEditor.getByLabelText("Description (optional)")).toHaveValue("New unsaved note");
    expect(secondEditor.getByRole("button", { name: "Save changes" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Reload latest" })).toBeEnabled();
  });

  it("sends the draft's original version even after an import refreshes the list", async () => {
    const user = userEvent.setup();
    render(<App />);
    const editor = await openEditor(user);
    await user.clear(editor.getByLabelText("Description (optional)"));
    await user.type(editor.getByLabelText("Description (optional)"), "My draft description");

    await user.click(await screen.findByRole("button", { name: "Import again" }));
    const summary = screen.getByRole("heading", { name: "Summary" }).closest(".card") as HTMLElement;
    await waitFor(() => {
      const moneyOutLabel = within(summary).getByText("Money out");
      const moneyOutTile = moneyOutLabel.closest(".metricTile");
      expect(moneyOutTile).toHaveTextContent("$12.00");
    });
    expect(editor.getByLabelText("Amount")).toHaveValue(-10);
    await user.click(editor.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(updateBodies).toHaveLength(1));
    expect(updateBodies[0]).toEqual(expect.objectContaining({
      amount: "-10.00",
      date: monthValue() + "-01",
      description: "My draft description",
      expected_version: 1,
    }));
    expect(savedTransaction.amount).toBe("-12.00");
  });

  it("preserves a conflicting draft until the user explicitly reloads and reviews the latest transaction", async () => {
    const user = userEvent.setup();
    render(<App />);
    const editor = await openEditor(user);
    await user.clear(editor.getByLabelText("Description (optional)"));
    await user.type(editor.getByLabelText("Description (optional)"), "My unsaved note");

    // another tab/backend completes an import without refreshing this tab's draft.
    savedTransaction = {
      ...savedTransaction,
      amount: "-12.00",
      date: monthValue() + "-02",
      row_version: 2,
    };
    await user.click(editor.getByRole("button", { name: "Save changes" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Transaction changed while you were editing");
    expect(editor.getByLabelText("Description (optional)")).toHaveValue("My unsaved note");
    expect(editor.getByLabelText("Amount")).toHaveValue(-10);
    expect(screen.queryByText("Transaction updated successfully.")).not.toBeInTheDocument();
    expect(updateBodies).toHaveLength(1);
    expect(savedTransaction.amount).toBe("-12.00");

    await user.click(screen.getByRole("button", { name: /reload latest/i }));
    const reloadedForm = screen.getByRole("form", { name: "Edit transaction" });
    const reloadedRow = reloadedForm.closest("tr") as HTMLElement;
    const reloaded = within(reloadedRow);
    await waitFor(() => expect(reloaded.getByLabelText("Amount")).toHaveValue(-12));
    expect(reloaded.getByLabelText("Date")).toHaveValue(monthValue() + "-02");
    expect(reloaded.getByLabelText("Description (optional)")).toHaveValue("Imported coffee");
    expect(updateBodies).toHaveLength(1);

    await user.clear(reloaded.getByLabelText("Description (optional)"));
    await user.type(reloaded.getByLabelText("Description (optional)"), "Reviewed note");
    await user.click(reloaded.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(updateBodies).toHaveLength(2));
    expect(updateBodies[1]).toEqual(expect.objectContaining({
      amount: "-12.00",
      date: monthValue() + "-02",
      description: "Reviewed note",
      expected_version: 2,
    }));
    expect(await screen.findByText("Transaction updated successfully.")).toBeInTheDocument();
  });

  it.each(["import-first", "reload-first"])("completes an explicit reload superseded by an import refresh (%s)", async (responseOrder) => {
    const user = userEvent.setup();
    render(<App />);
    const editor = await openEditor(user);
    await user.clear(editor.getByLabelText("Description (optional)"));
    await user.type(editor.getByLabelText("Description (optional)"), "Unsaved note");
    savedTransaction = {
      ...savedTransaction,
      amount: "-12.00",
      date: monthValue() + "-02",
      row_version: 2,
    };
    await user.click(editor.getByRole("button", { name: "Save changes" }));
    await screen.findByRole("alert");

    const importReply = deferred<Response>();
    const originalReloadReply = deferred<Response>();
    const importRefreshReply = deferred<Response>();
    const originalReloadSnapshot = { ...savedTransaction };
    const initialFetch = vi.mocked(fetch).getMockImplementation()!;
    let transactionReads = 0;
    vi.mocked(fetch).mockImplementation(async (input, init) => {
      if (String(input) === "/api/bank-connections/row-version-bank/import") return importReply.promise;
      if (String(input) === "/api/transactions" && init?.method === "GET") {
        transactionReads++;
        if (transactionReads === 1) return originalReloadReply.promise;
        if (transactionReads === 2) return importRefreshReply.promise;
      }
      return initialFetch(input, init);
    });

    try {
      await user.click(screen.getByRole("button", { name: "Import again" }));
      await user.click(screen.getByRole("button", { name: "Reload latest" }));
      await waitFor(() => expect(transactionReads).toBe(1));
      expect(screen.getByRole("button", { name: "Reloading…" })).toBeDisabled();
      expect(editor.getByLabelText("Description (optional)")).toHaveValue("Unsaved note");

      savedTransaction = {
        ...savedTransaction,
        amount: "-13.00",
        date: monthValue() + "-03",
        row_version: 3,
      };
      await act(async () => {
        const response = jsonResponse({ imported: 0 });
        importReply.resolve(response);
      });
      await waitFor(() => expect(transactionReads).toBe(2));
      if (responseOrder === "import-first") {
        await act(async () => {
          const response = jsonResponse([savedTransaction]);
          importRefreshReply.resolve(response);
        });
        await act(async () => {
          const response = jsonResponse([originalReloadSnapshot]);
          originalReloadReply.resolve(response);
        });
      } else {
        await act(async () => {
          const response = jsonResponse([originalReloadSnapshot]);
          originalReloadReply.resolve(response);
        });
        await act(async () => {
          const response = jsonResponse([savedTransaction]);
          importRefreshReply.resolve(response);
        });
      }

      await waitFor(() => {
        const editorForm = screen.getByRole("form", { name: "Edit transaction" });
        const editorRow = editorForm.closest("tr") as HTMLElement;
        const currentEditor = within(editorRow);
        expect(currentEditor.getByLabelText("Amount")).toHaveValue(-13);
      });
      const reloadedForm = screen.getByRole("form", { name: "Edit transaction" });
      const reloadedRow = reloadedForm.closest("tr") as HTMLElement;
      const reloaded = within(reloadedRow);
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
        amount: "-13.00",
        date: monthValue() + "-03",
        expected_version: 3,
        description: "Reviewed latest import",
      }));
    } finally {
      await act(async () => {
        importReply.resolve(jsonResponse({ imported: 0 }));
        originalReloadReply.resolve(jsonResponse([originalReloadSnapshot]));
        importRefreshReply.resolve(jsonResponse([savedTransaction]));
      });
    }
  });
});

async function openConflictingDraft(user: ReturnType<typeof userEvent.setup>) {
  render(<App />);
  const editor = await openEditor(user);
  await user.clear(editor.getByLabelText("Description (optional)"));
  await user.type(editor.getByLabelText("Description (optional)"), "Unsaved note");
  savedTransaction = {
    ...savedTransaction,
    amount: "-12.00",
    date: monthValue() + "-02",
    row_version: 2,
  };
  await user.click(editor.getByRole("button", { name: "Save changes" }));
  await screen.findByRole("alert");
}

// hold the GET started by reload latest, then control the newer GET started by import again.
// keep the first response pending until the test has checked recovery, errors, or logout.
function holdOriginalReload(importRefreshOutcome: "success" | "http-error" | "network-error" | "removed" | "pending" = "success") {
  const originalReloadReply = deferred<Response>();
  const importRefreshReply = deferred<Response>();
  const originalReloadSnapshot = { ...savedTransaction };
  const initialFetch = vi.mocked(fetch).getMockImplementation()!;
  let transactionReads = 0;
  let originalReloadSettled = false;
  void originalReloadReply.promise.then(() => {
    originalReloadSettled = true;
  });
  vi.mocked(fetch).mockImplementation(async (input, init) => {
    if (String(input) === "/api/bank-connections/row-version-bank/import") {
      savedTransaction = {
        ...savedTransaction,
        amount: "-13.00",
        date: monthValue() + "-03",
        row_version: 3,
      };
      return jsonResponse({ imported: 0 });
    }
    if (String(input) === "/api/transactions" && init?.method === "GET") {
      transactionReads++;
      if (transactionReads === 1) return originalReloadReply.promise;
      if (transactionReads === 2) {
        if (importRefreshOutcome === "http-error") return jsonResponse({ error: "Latest refresh failed" }, 500);
        if (importRefreshOutcome === "network-error") throw new TypeError("Latest refresh failed");
        if (importRefreshOutcome === "removed") return jsonResponse([]);
        if (importRefreshOutcome === "pending") return importRefreshReply.promise;
      }
    }
    return initialFetch(input, init);
  });
  return {
    originalReloadReply,
    importRefreshReply,
    originalReloadSnapshot,
    transactionReads: () => transactionReads,
    originalReloadSettled: () => originalReloadSettled,
    settlePendingResponses: async () => act(async () => {
      originalReloadReply.resolve(jsonResponse([originalReloadSnapshot]));
      importRefreshReply.resolve(jsonResponse([savedTransaction]));
    }),
  };
}

describe("reload recovery while an obsolete read is still pending", () => {
  it.each([200, 500])("recovers and saves before the obsolete read settles, ignoring its later %s response", async (lateResponseStatus) => {
    const user = userEvent.setup();
    await openConflictingDraft(user);
    const reloadResponses = holdOriginalReload();
    try {
      await user.click(screen.getByRole("button", { name: "Reload latest" }));
      await waitFor(() => expect(reloadResponses.transactionReads()).toBe(1));
      await user.click(screen.getByRole("button", { name: "Import again" }));
      await waitFor(() => expect(reloadResponses.transactionReads()).toBe(2));
      const summary = screen.getByRole("heading", { name: "Summary" }).closest(".card") as HTMLElement;
      await waitFor(() => {
        const moneyOutLabel = within(summary).getByText("Money out");
        const moneyOutTile = moneyOutLabel.closest(".metricTile");
        expect(moneyOutTile).toHaveTextContent("$13.00");
      });

      // the obsolete response remains unresolved through recovery and the reviewed save.
      await waitFor(() => {
        const editorForm = screen.getByRole("form", { name: "Edit transaction" });
        const editorRow = editorForm.closest("tr") as HTMLElement;
        const currentEditor = within(editorRow);
        expect(currentEditor.getByLabelText("Amount")).toHaveValue(-13);
      });
      expect(reloadResponses.originalReloadSettled()).toBe(false);
      const editorForm = screen.getByRole("form", { name: "Edit transaction" });
      const editorRow = editorForm.closest("tr") as HTMLElement;
      const editor = within(editorRow);
      expect(editor.getByLabelText("Date")).toHaveValue(monthValue() + "-03");
      expect(editor.getByRole("button", { name: "Save changes" })).toBeEnabled();
      expect(editor.getByRole("button", { name: "Cancel" })).toBeEnabled();
      expect(updateBodies).toHaveLength(1);
      await user.clear(editor.getByLabelText("Description (optional)"));
      await user.type(editor.getByLabelText("Description (optional)"), "Reviewed current values");
      await user.click(editor.getByRole("button", { name: "Save changes" }));
      await screen.findByText("Transaction updated successfully.");
      expect(reloadResponses.originalReloadSettled()).toBe(false);
      expect(updateBodies[1]).toEqual(expect.objectContaining({
        amount: "-13.00",
        date: monthValue() + "-03",
        expected_version: 3,
      }));

      await act(async () => {
        const body = lateResponseStatus === 200
          ? [reloadResponses.originalReloadSnapshot]
          : { error: "Obsolete read failed" };
        const response = jsonResponse(body, lateResponseStatus);
        reloadResponses.originalReloadReply.resolve(response);
      });
      expect(screen.queryByRole("form", { name: "Edit transaction" })).not.toBeInTheDocument();
      expect(screen.queryByText("Obsolete read failed")).not.toBeInTheDocument();
      expect(screen.getByText("Reviewed current values")).toBeInTheDocument();
    } finally {
      await reloadResponses.settlePendingResponses();
    }
  });

  it.each(["http-error", "network-error"] as const)("reports the current %s without waiting for the obsolete read and allows an explicit retry", async (failure) => {
    const user = userEvent.setup();
    await openConflictingDraft(user);
    const reloadResponses = holdOriginalReload(failure);
    try {
      await user.click(screen.getByRole("button", { name: "Reload latest" }));
      await waitFor(() => expect(reloadResponses.transactionReads()).toBe(1));
      await user.click(screen.getByRole("button", { name: "Import again" }));
      await screen.findByText("Latest refresh failed");
      expect(reloadResponses.originalReloadSettled()).toBe(false);
      expect(reloadResponses.transactionReads()).toBe(2);
      const editorForm = screen.getByRole("form", { name: "Edit transaction" });
      const editorRow = editorForm.closest("tr") as HTMLElement;
      const editor = within(editorRow);
      expect(editor.getByLabelText("Amount")).toHaveValue(-10);
      expect(editor.getByLabelText("Description (optional)")).toHaveValue("Unsaved note");
      expect(editor.getByRole("button", { name: "Save changes" })).toBeDisabled();
      expect(screen.getByRole("button", { name: "Reload latest" })).toBeEnabled();
      await user.click(screen.getByRole("button", { name: "Reload latest" }));
      await waitFor(() => {
        const editorForm = screen.getByRole("form", { name: "Edit transaction" });
        const editorRow = editorForm.closest("tr") as HTMLElement;
        const currentEditor = within(editorRow);
        expect(currentEditor.getByLabelText("Amount")).toHaveValue(-13);
      });
      expect(reloadResponses.originalReloadSettled()).toBe(false);
      expect(updateBodies).toHaveLength(1);
    } finally {
      await reloadResponses.settlePendingResponses();
    }
  });

  it("closes an unavailable editor before the obsolete read settles", async () => {
    const user = userEvent.setup();
    await openConflictingDraft(user);
    const reloadResponses = holdOriginalReload("removed");
    try {
      await user.click(screen.getByRole("button", { name: "Reload latest" }));
      await waitFor(() => expect(reloadResponses.transactionReads()).toBe(1));
      await user.click(screen.getByRole("button", { name: "Import again" }));
      await screen.findByText("Transaction is no longer available.");
      expect(reloadResponses.originalReloadSettled()).toBe(false);
      expect(screen.queryByRole("form", { name: "Edit transaction" })).not.toBeInTheDocument();
      expect(updateBodies).toHaveLength(1);
    } finally {
      await reloadResponses.settlePendingResponses();
    }
  });

  it.each([200, 500, 401])("ignores old pending reads with a late %s after logout and a new session's pending save", async (lateResponseStatus) => {
    const user = userEvent.setup();
    await openConflictingDraft(user);
    const reloadResponses = holdOriginalReload("pending");
    const writeReply = deferred<Response>();
    try {
      await user.click(screen.getByRole("button", { name: "Reload latest" }));
      await waitFor(() => expect(reloadResponses.transactionReads()).toBe(1));
      await user.click(screen.getByRole("button", { name: "Import again" }));
      await waitFor(() => expect(reloadResponses.transactionReads()).toBe(2));
      await user.click(screen.getByRole("button", { name: "Logout" }));
      expect(localStorage.getItem("access_token")).toBeNull();
      const nextPayload = btoa(JSON.stringify({ exp: Math.floor(Date.now() / 1000) + 7200 }));
      const nextToken = `header.${nextPayload}.signature`;
      let nextTransaction = {
        ...originalTransaction(),
        id: "next-coffee",
        amount: "-7.00",
        description: "Next user coffee",
      };
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
          nextTransaction = {
            ...nextTransaction,
            ...nextSave,
            row_version: nextTransaction.row_version + 1,
          };
          return writeReply.promise;
        }
        return previousFetch(input, init);
      });
      await user.type(screen.getByLabelText("Username or Email"), "next-user");
      await user.type(screen.getByLabelText("Password"), "password{Enter}");
      await user.click(await screen.findByRole("button", { name: /Edit transaction: Next user coffee/ }));
      const editorForm = screen.getByRole("form", { name: "Edit transaction" });
      const editorRow = editorForm.closest("tr") as HTMLElement;
      const editor = within(editorRow);
      await user.clear(editor.getByLabelText("Description (optional)"));
      await user.type(editor.getByLabelText("Description (optional)"), "Next draft");
      await user.click(editor.getByRole("button", { name: "Save changes" }));
      await waitFor(() => expect(nextSave).toBeDefined());
      const transactionReadsBeforeLateResponses = vi.mocked(fetch).mock.calls.filter(
        ([input, init]) => String(input) === "/api/transactions" && init?.method === "GET",
      );
      await act(async () => {
        const originalResponse = jsonResponse([reloadResponses.originalReloadSnapshot]);
        reloadResponses.originalReloadReply.resolve(originalResponse);
        const body = lateResponseStatus === 200
          ? [savedTransaction]
          : { error: "Old session failed" };
        const response = jsonResponse(body, lateResponseStatus);
        reloadResponses.importRefreshReply.resolve(response);
      });
      expect(localStorage.getItem("access_token")).toBe(nextToken);
      expect(editor.getByLabelText("Description (optional)")).toHaveValue("Next draft");
      expect(editor.getByRole("button", { name: "Saving…" })).toBeDisabled();
      expect(screen.queryByText("Old session failed")).not.toBeInTheDocument();
      const transactionReadsAfterLateResponses = vi.mocked(fetch).mock.calls.filter(
        ([input, init]) => String(input) === "/api/transactions" && init?.method === "GET",
      );
      expect(transactionReadsAfterLateResponses).toHaveLength(transactionReadsBeforeLateResponses.length);
      await act(async () => {
        const response = new Response(null, { status: 200 });
        writeReply.resolve(response);
      });
      await screen.findByText("Transaction updated successfully.");
    } finally {
      await reloadResponses.settlePendingResponses();
      await act(async () => {
        const response = new Response(null, { status: 200 });
        writeReply.resolve(response);
      });
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
    const editorForm = screen.getByRole("form", { name: "Edit transaction" });
    const editorRow = editorForm.closest("tr") as HTMLElement;
    const editor = within(editorRow);
    expect(editor.getByLabelText("Description (optional)")).toHaveValue("Unsaved note");
    expect(editor.getByRole("button", { name: "Save changes" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Reload latest" })).toBeEnabled();
    await user.click(screen.getByRole("button", { name: "Reload latest" }));
    await waitFor(() => {
      const editorForm = screen.getByRole("form", { name: "Edit transaction" });
      const editorRow = editorForm.closest("tr") as HTMLElement;
      const currentEditor = within(editorRow);
      expect(currentEditor.getByLabelText("Amount")).toHaveValue(-12);
    });
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
