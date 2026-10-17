import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PlaidLinkOptions, PlaidLinkOnSuccessMetadata, PlaidLinkOnExitMetadata } from "react-plaid-link";
import App from "./App";
import type { BankConnectionResponse } from "./types";

const api = vi.hoisted(() => ({
  addTransaction: vi.fn(), getBudgetProgress: vi.fn(), getBudgets: vi.fn(), getTransactions: vi.fn(),
  loginUser: vi.fn(), registerUser: vi.fn(), semanticSearchTransactions: vi.fn(),
  setUnauthorizedCallback: vi.fn(), upsertBudget: vi.fn(),
  createBankLinkToken: vi.fn(), exchangeBankToken: vi.fn(), importBankTransactions: vi.fn(),
  getBankConnections: vi.fn(),
  getIndexingStatus: vi.fn(),
}));

// retain Plaid's real callback contract while keeping its remote UI and all network calls out of these tests
const plaid = vi.hoisted(() => ({
  options: null as PlaidLinkOptions | null,
  ready: true,
  error: null as Error | null,
  open: vi.fn(),
}));
vi.mock("./api", () => api);
vi.mock("react-plaid-link", () => ({
  usePlaidLink: (options: PlaidLinkOptions) => {
    plaid.options = options;
    return { ready: plaid.ready, error: plaid.error, open: plaid.open, exit: vi.fn() };
  },
}));

function sessionToken() {
  return `header.${btoa(JSON.stringify({ exp: Math.floor(Date.now() / 1000) + 3600 }))}.signature`;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((finish) => { resolve = finish; });
  return { promise, resolve };
}

async function startLink(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: "Connect bank" }));
  await waitFor(() => expect(plaid.options?.token).toBe("link-test"));
  return plaid.options!;
}

async function finishLink(options = plaid.options!) {
  await act(async () => options.onSuccess("public-test", {} as PlaidLinkOnSuccessMetadata));
}

beforeEach(() => {
  vi.resetAllMocks();
  plaid.options = null;
  plaid.ready = true;
  plaid.error = null;
  localStorage.setItem("access_token", sessionToken());
  api.getTransactions.mockResolvedValue([]);
  api.getBudgets.mockResolvedValue([]);
  api.getBudgetProgress.mockResolvedValue([]);
  api.getBankConnections.mockResolvedValue([]);
  api.getIndexingStatus.mockResolvedValue({ total: 0, indexed: 0, remaining: 0 });
  api.createBankLinkToken.mockResolvedValue({ link_token: "link-test", expiration: "2026-10-16T20:00:00Z" });
  api.exchangeBankToken.mockResolvedValue({ connection_id: "saved-connection", provider: "plaid", status: "active" });
  api.importBankTransactions.mockResolvedValue({ imported: 12 });
});

describe("Connect bank", () => {
  it("requests a token on click and opens Plaid once its SDK is ready", async () => {
    const user = userEvent.setup();
    plaid.ready = false;
    const view = render(<App />);
    expect(api.createBankLinkToken).not.toHaveBeenCalled();

    await startLink(user);
    expect(api.createBankLinkToken).toHaveBeenCalledOnce();
    expect(plaid.open).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Connect bank" })).toBeDisabled();

    // re-rendering the SDK as ready must open this attempt only once
    plaid.ready = true;
    view.rerender(<App />);
    await waitFor(() => expect(plaid.open).toHaveBeenCalledOnce());
    view.rerender(<App />);
    expect(plaid.open).toHaveBeenCalledOnce();
  });

  it("exchanges Plaid's result, imports that connection, and refreshes the dashboard", async () => {
    const user = userEvent.setup();
    render(<App />);
    await waitFor(() => expect(api.getBudgetProgress).toHaveBeenCalledOnce());
    const month = new Date();
    const date = `${month.getFullYear()}-${String(month.getMonth() + 1).padStart(2, "0")}-01`;
    api.getTransactions.mockResolvedValue([{ id: "imported-transaction", amount: "-12.50", kind: "Expense", category: "Food", date, description: "Imported grocery purchase" }]);
    api.getBudgetProgress.mockResolvedValue([{ category: "Food", budget_amount: "100.00", spent: "12.50", remaining: "87.50" }]);

    await startLink(user);
    await finishLink();

    expect(api.exchangeBankToken).toHaveBeenCalledExactlyOnceWith("public-test");
    expect(api.importBankTransactions).toHaveBeenCalledExactlyOnceWith("saved-connection");
    expect(api.exchangeBankToken.mock.invocationCallOrder[0]).toBeLessThan(api.importBankTransactions.mock.invocationCallOrder[0]);
    expect(await screen.findByText("Imported 12 transactions.")).toBeInTheDocument();
    expect(await screen.findByText("Imported grocery purchase")).toBeInTheDocument();
    await waitFor(() => expect(api.getBudgetProgress).toHaveBeenCalledTimes(2));
    expect(api.getTransactions).toHaveBeenCalledTimes(2);
    expect(api.getBudgets).toHaveBeenCalledTimes(2);
  });

  it("treats closing Plaid as cancellation and ignores callbacks from that cancelled attempt", async () => {
    const user = userEvent.setup();
    render(<App />);
    const cancelled = await startLink(user);
    act(() => cancelled.onExit?.(null, {} as PlaidLinkOnExitMetadata));
    expect(screen.getByRole("button", { name: "Connect bank" })).toBeEnabled();
    expect(api.exchangeBankToken).not.toHaveBeenCalled();
    expect(api.importBankTransactions).not.toHaveBeenCalled();

    // an old success arriving after a fresh attempt starts must not exchange the wrong public token
    await startLink(user);
    await finishLink(cancelled);
    expect(api.exchangeBankToken).not.toHaveBeenCalled();
    await finishLink();
    expect(api.exchangeBankToken).toHaveBeenCalledOnce();
  });

  it("shows a token-request failure and allows a fresh Connect bank attempt", async () => {
    const user = userEvent.setup();
    api.createBankLinkToken.mockRejectedValueOnce(new Error("Bank linking is not configured"));
    render(<App />);
    await user.click(screen.getByRole("button", { name: "Connect bank" }));

    expect(await screen.findByText(/Bank linking is not configured/)).toBeInTheDocument();
    expect(plaid.open).not.toHaveBeenCalled();
    expect(api.exchangeBankToken).not.toHaveBeenCalled();
    await startLink(user);
    expect(api.createBankLinkToken).toHaveBeenCalledTimes(2);
  });

  it("reports an SDK load failure and enables another attempt", async () => {
    const user = userEvent.setup();
    plaid.error = new Error("Script failed");
    render(<App />);
    await user.click(screen.getByRole("button", { name: "Connect bank" }));

    expect(await screen.findByText(/Could not open Plaid/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Connect bank" })).toBeEnabled();
    expect(plaid.open).not.toHaveBeenCalled();
    expect(api.exchangeBankToken).not.toHaveBeenCalled();
  });

  it("shows a Plaid error on exit without exchanging or importing", async () => {
    const user = userEvent.setup();
    render(<App />);
    const options = await startLink(user);
    act(() => options.onExit?.({ error_type: "ITEM_ERROR", error_code: "TEST_ERROR", error_message: "Provider error", display_message: null }, {} as PlaidLinkOnExitMetadata));

    expect(await screen.findByText(/Plaid could not complete bank linking/)).toBeInTheDocument();
    expect(api.exchangeBankToken).not.toHaveBeenCalled();
    expect(api.importBankTransactions).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Connect bank" })).toBeEnabled();
  });

  it("stops after a failed exchange and allows linking again", async () => {
    const user = userEvent.setup();
    api.exchangeBankToken.mockRejectedValueOnce(new Error("Could not save bank connection"));
    render(<App />);
    await startLink(user);
    await finishLink();

    expect(await screen.findByText(/Could not save bank connection/)).toBeInTheDocument();
    expect(api.importBankTransactions).not.toHaveBeenCalled();
    expect(screen.queryByText(/Imported 12/)).not.toBeInTheDocument();
    await startLink(user);
    expect(api.createBankLinkToken).toHaveBeenCalledTimes(2);
  });

  it("retries a failed import using the saved connection without linking again", async () => {
    const user = userEvent.setup();
    api.importBankTransactions.mockRejectedValueOnce(new Error("Could not fetch bank transactions"));
    render(<App />);
    await startLink(user);
    await finishLink();
    expect(await screen.findByText(/Bank connected.*Could not fetch bank transactions/)).toBeInTheDocument();

    // the exchange already saved this connection, so retry only the import endpoint
    await user.click(screen.getByRole("button", { name: "Retry import" }));
    expect(await screen.findByText("Imported 12 transactions.")).toBeInTheDocument();
    expect(api.importBankTransactions).toHaveBeenNthCalledWith(2, "saved-connection");
    expect(api.exchangeBankToken).toHaveBeenCalledOnce();
    expect(api.createBankLinkToken).toHaveBeenCalledOnce();
  });

  it("shows zero results honestly and allows another import for that connection", async () => {
    const user = userEvent.setup();
    api.importBankTransactions.mockResolvedValueOnce({ imported: 0 }).mockResolvedValueOnce({ imported: 1 });
    render(<App />);
    await startLink(user);
    await finishLink();

    expect(await screen.findByText("Connected; no transactions imported yet.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Import again" }));
    expect(await screen.findByText("Imported 1 transaction.")).toBeInTheDocument();
    expect(api.importBankTransactions).toHaveBeenCalledTimes(2);
    expect(api.exchangeBankToken).toHaveBeenCalledOnce();
  });

  it("blocks duplicate clicks and repeated Plaid success callbacks while work is pending", async () => {
    const user = userEvent.setup();
    const token = deferred<{ link_token: string; expiration: string }>();
    const exchange = deferred<{ connection_id: string; provider: string; status: string }>();
    const imported = deferred<{ imported: number }>();
    api.createBankLinkToken.mockReturnValue(token.promise);
    api.exchangeBankToken.mockReturnValue(exchange.promise);
    api.importBankTransactions.mockReturnValue(imported.promise);
    render(<App />);

    const button = screen.getByRole("button", { name: "Connect bank" });
    await user.dblClick(button);
    expect(api.createBankLinkToken).toHaveBeenCalledOnce();
    await act(async () => token.resolve({ link_token: "link-test", expiration: "2026-10-16T20:00:00Z" }));
    const options = plaid.options!;
    await finishLink(options);
    await finishLink(options);
    expect(api.exchangeBankToken).toHaveBeenCalledOnce();
    expect(button).toBeDisabled();

    await act(async () => exchange.resolve({ connection_id: "saved-connection", provider: "plaid", status: "active" }));
    expect(api.importBankTransactions).toHaveBeenCalledOnce();
    await finishLink(options);
    await user.click(button);
    expect(api.exchangeBankToken).toHaveBeenCalledOnce();
    await act(async () => imported.resolve({ imported: 12 }));
    expect(await screen.findByText("Imported 12 transactions.")).toBeInTheDocument();
    expect(button).toBeEnabled();
  });

  it("ignores a Plaid success callback after logout", async () => {
    const user = userEvent.setup();
    render(<App />);
    const options = await startLink(user);
    await user.click(screen.getByRole("button", { name: "Logout" }));
    await finishLink(options);

    expect(screen.getByRole("heading", { name: "FinanceTracker" })).toBeInTheDocument();
    expect(api.exchangeBankToken).not.toHaveBeenCalled();
    expect(api.importBankTransactions).not.toHaveBeenCalled();
  });

  it.each(["token", "exchange", "import"] as const)("stops an unfinished %s response from continuing after logout", async (stage) => {
    const user = userEvent.setup();
    const pending = deferred<unknown>();
    const request = stage === "token" ? api.createBankLinkToken : stage === "exchange" ? api.exchangeBankToken : api.importBankTransactions;
    request.mockReturnValue(pending.promise);
    render(<App />);
    await waitFor(() => expect(api.getBudgetProgress).toHaveBeenCalledOnce());

    await user.click(screen.getByRole("button", { name: "Connect bank" }));
    if (stage !== "token") {
      await waitFor(() => expect(plaid.options).not.toBeNull());
      await finishLink();
      await waitFor(() => expect(request).toHaveBeenCalledOnce());
    }
    await user.click(screen.getByRole("button", { name: "Logout" }));
    const result = stage === "token" ? { link_token: "link-test", expiration: "2026-10-16T20:00:00Z" }
      : stage === "exchange" ? { connection_id: "saved-connection", provider: "plaid", status: "active" } : { imported: 12 };
    await act(async () => pending.resolve(result));

    // a response belonging to the previous session cannot open Plaid, start another write, or refresh user data
    if (stage === "token") expect(plaid.open).not.toHaveBeenCalled();
    if (stage !== "import") expect(api.importBankTransactions).not.toHaveBeenCalled();
    expect(api.getTransactions).toHaveBeenCalledOnce();
    expect(api.getBudgetProgress).toHaveBeenCalledOnce();
    expect(screen.queryByText(/Imported 12/)).not.toBeInTheDocument();
  });

  it("does not apply a completed dashboard refresh after logout and another login", async () => {
    const user = userEvent.setup();
    render(<App />);
    await waitFor(() => expect(api.getBudgetProgress).toHaveBeenCalledOnce());
    const oldRefresh = deferred<unknown[]>();
    api.getTransactions.mockReturnValueOnce(oldRefresh.promise);
    await startLink(user);
    await finishLink();
    await waitFor(() => expect(api.getTransactions).toHaveBeenCalledTimes(2));

    await user.click(screen.getByRole("button", { name: "Logout" }));
    api.loginUser.mockResolvedValue({ access_token: sessionToken(), user_id: "other-user" });
    await user.type(screen.getByLabelText("Username or Email"), "other-user");
    await user.type(screen.getByLabelText("Password"), "password{Enter}");
    await screen.findByRole("heading", { name: "Dashboard" });
    await act(async () => oldRefresh.resolve([{ id: "old-user-row", amount: "-1.00", kind: "Expense", category: null, date: "2026-10-01", description: "Previous user's transaction" }]));

    expect(screen.queryByText("Previous user's transaction")).not.toBeInTheDocument();
    expect(screen.queryByText(/Imported 12/)).not.toBeInTheDocument();
  });

  it("keeps an import success distinct from a failed dashboard refresh", async () => {
    const user = userEvent.setup();
    render(<App />);
    await waitFor(() => expect(api.getBudgetProgress).toHaveBeenCalledOnce());
    api.getTransactions.mockRejectedValueOnce(new Error("Display refresh failed"));
    await startLink(user);
    await finishLink();

    expect(await screen.findByText(/Imported 12 transactions.*could not refresh/i)).toBeInTheDocument();
    expect(api.importBankTransactions).toHaveBeenCalledOnce();
    expect(screen.getByRole("button", { name: "Connect bank" })).toBeEnabled();
  });
});


const restoredConnections: BankConnectionResponse[] = [
  { connection_id: "restored-first", provider: "plaid", status: "active" },
  { connection_id: "restored-second", provider: "plaid", status: "active" },
];

describe("Saved bank connections", () => {
  it("loads saved connections again after a reload without linking or importing", async () => {
    api.getBankConnections.mockResolvedValue(restoredConnections);
    const view = render(<App />);
    await screen.findByRole("group", { name: "Plaid connection 2" });
    expect(api.getBankConnections).toHaveBeenCalledOnce();

    // Remount the app with the same login, as a reload would, rather than preserving component state.
    view.unmount();
    render(<App />);
    await screen.findByRole("group", { name: "Plaid connection 2" });
    expect(api.getBankConnections).toHaveBeenCalledTimes(2);
    expect(screen.getAllByRole("button", { name: "Import again" })).toHaveLength(2);

    // Loading the saved list must not open Plaid, exchange a token, or start an import.
    expect(plaid.open).not.toHaveBeenCalled();
    expect(api.createBankLinkToken).not.toHaveBeenCalled();
    expect(api.exchangeBankToken).not.toHaveBeenCalled();
    expect(api.importBankTransactions).not.toHaveBeenCalled();
  });

  it("imports the selected saved connection once and refreshes transactions and budgets", async () => {
    const user = userEvent.setup();
    const pending = deferred<{ imported: number }>();
    api.getBankConnections.mockResolvedValue(restoredConnections);
    api.importBankTransactions.mockReturnValueOnce(pending.promise);
    render(<App />);
    const second = await screen.findByRole("group", { name: "Plaid connection 2" });
    await waitFor(() => expect(api.getBudgetProgress).toHaveBeenCalledOnce());

    // Supply the dashboard data that will be fetched after the import completes.
    const month = new Date();
    const date = `${month.getFullYear()}-${String(month.getMonth() + 1).padStart(2, "0")}-01`;
    api.getTransactions.mockResolvedValue([{ id: "restored-import", amount: "-12.50", kind: "Expense", category: "Food", date, description: "Purchase from saved bank" }]);
    api.getBudgetProgress.mockResolvedValue([{ category: "Food", budget_amount: "100.00", spent: "12.50", remaining: "87.50" }]);

    // Two clicks while the first import is pending must still submit only this connection once.
    await user.dblClick(within(second).getByRole("button", { name: "Import again" }));
    expect(api.importBankTransactions).toHaveBeenCalledExactlyOnceWith("restored-second");
    expect(screen.getByRole("button", { name: "Connect bank" })).toBeDisabled();
    for (const button of screen.getAllByRole("button", { name: "Import again" })) {
      expect(button).toBeDisabled();
    }
    expect(api.createBankLinkToken).not.toHaveBeenCalled();
    expect(api.exchangeBankToken).not.toHaveBeenCalled();

    // The successful count and refreshed dashboard must refer to the completed import.
    await act(async () => pending.resolve({ imported: 1 }));
    expect(await screen.findByText("Imported 1 transaction.")).toBeInTheDocument();
    expect(await screen.findByText("Purchase from saved bank")).toBeInTheDocument();
    await waitFor(() => expect(api.getBudgetProgress).toHaveBeenCalledTimes(2));
    expect(api.getTransactions).toHaveBeenCalledTimes(2);
    expect(api.getBudgets).toHaveBeenCalledTimes(2);
    expect(within(second).getByRole("button", { name: "Import again" })).toBeEnabled();
  });

  it("shows an empty saved list and keeps Connect bank available", async () => {
    render(<App />);
    expect(await screen.findByText("No saved bank connections.")).toBeInTheDocument();
    expect(api.getBankConnections).toHaveBeenCalledOnce();
    expect(screen.getByRole("button", { name: "Connect bank" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Import again" })).not.toBeInTheDocument();
    expect(api.importBankTransactions).not.toHaveBeenCalled();
  });

  it("shows inactive connection statuses without offering imports for them", async () => {
    api.getBankConnections.mockResolvedValue([
      restoredConnections[0],
      { connection_id: "needs-login", provider: "plaid", status: "needs_reconnection" },
      { connection_id: "disconnected", provider: "plaid", status: "disconnected" },
    ]);
    render(<App />);
    const needsLogin = await screen.findByRole("group", { name: "Plaid connection 2" });
    const disconnected = screen.getByRole("group", { name: "Plaid connection 3" });

    // Preserve the server's status instead of presenting every saved record as importable.
    expect(within(needsLogin).getByText("Needs reconnection")).toBeInTheDocument();
    expect(within(disconnected).getByText("Disconnected")).toBeInTheDocument();
    expect(within(needsLogin).queryByRole("button")).not.toBeInTheDocument();
    expect(within(disconnected).queryByRole("button")).not.toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Import again" })).toHaveLength(1);
    expect(screen.getByRole("button", { name: "Connect bank" })).toBeEnabled();
    expect(api.importBankTransactions).not.toHaveBeenCalled();
  });

  it("retries loading a failed saved list without linking or importing", async () => {
    const user = userEvent.setup();
    api.getBankConnections.mockRejectedValueOnce(new Error("Service unavailable"))
      .mockResolvedValueOnce([restoredConnections[0]]);
    render(<App />);
    expect(await screen.findByRole("alert")).toHaveTextContent(/Could not load saved connections.*Service unavailable/);
    expect(screen.getByRole("button", { name: "Connect bank" })).toBeEnabled();

    // Retry only the failed read; it must not ask the user to link again or start an import.
    await user.click(screen.getByRole("button", { name: "Retry loading connections" }));
    await screen.findByRole("group", { name: "Plaid connection 1" });
    expect(api.getBankConnections).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Import again" })).toBeEnabled();
    expect(api.createBankLinkToken).not.toHaveBeenCalled();
    expect(api.exchangeBankToken).not.toHaveBeenCalled();
    expect(api.importBankTransactions).not.toHaveBeenCalled();
  });

  it("ignores the previous user's delayed list after logout and another login", async () => {
    const user = userEvent.setup();
    const oldList = deferred<BankConnectionResponse[]>();
    api.getBankConnections.mockReturnValueOnce(oldList.promise).mockResolvedValueOnce([
      { connection_id: "new-user-connection", provider: "plaid", status: "active" },
    ]);
    render(<App />);
    await waitFor(() => expect(api.getBankConnections).toHaveBeenCalledOnce());

    // Change users while the original list is still pending, then wait for the new user's list.
    await user.click(screen.getByRole("button", { name: "Logout" }));
    api.loginUser.mockResolvedValue({ access_token: sessionToken(), user_id: "other-user" });
    await user.type(screen.getByLabelText("Username or Email"), "other-user");
    await user.type(screen.getByLabelText("Password"), "password{Enter}");
    await screen.findByRole("group", { name: "Plaid connection 1" });
    expect(api.getBankConnections).toHaveBeenCalledTimes(2);

    // The late response must neither add the old rows nor change which ID this button imports.
    await act(async () => oldList.resolve(restoredConnections));
    expect(screen.getAllByRole("group", { name: /^Plaid connection / })).toHaveLength(1);
    await user.click(screen.getByRole("button", { name: "Import again" }));
    expect(api.importBankTransactions).toHaveBeenCalledExactlyOnceWith("new-user-connection");
    expect(api.exchangeBankToken).not.toHaveBeenCalled();
  });

  it.each([false, true])("retains a newly linked connection when the delayed list includes it: %s", async (includesNewConnection) => {
    const user = userEvent.setup();
    const pendingList = deferred<BankConnectionResponse[]>();
    api.getBankConnections.mockReturnValueOnce(pendingList.promise);
    render(<App />);
    await waitFor(() => expect(api.getBankConnections).toHaveBeenCalledOnce());

    // Linking remains usable during loading; the newly saved connection imports normally.
    await startLink(user);
    await finishLink();
    expect(await screen.findByText("Imported 12 transactions.")).toBeInTheDocument();
    await screen.findByRole("group", { name: "Plaid connection 1" });

    // The delayed read may omit the new row or already contain it. Both must leave exactly two rows.
    const newlySaved = { connection_id: "saved-connection", provider: "plaid", status: "active" };
    await act(async () => pendingList.resolve([
      restoredConnections[0], ...(includesNewConnection ? [newlySaved] : []),
    ]));
    expect(screen.getAllByRole("group", { name: /^Plaid connection / })).toHaveLength(2);
    const newlyLinked = screen.getByRole("group", { name: "Plaid connection 2" });
    await user.click(within(newlyLinked).getByRole("button", { name: "Import again" }));
    expect(api.importBankTransactions).toHaveBeenNthCalledWith(2, "saved-connection");
    expect(api.exchangeBankToken).toHaveBeenCalledOnce();
  });

  it("retries a failed import for the restored connection without linking again", async () => {
    const user = userEvent.setup();
    api.getBankConnections.mockResolvedValue(restoredConnections);
    api.importBankTransactions.mockRejectedValueOnce(new Error("Import unavailable"));
    render(<App />);
    const second = await screen.findByRole("group", { name: "Plaid connection 2" });
    await user.click(within(second).getByRole("button", { name: "Import again" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/Bank connected.*Import unavailable/);

    // Keep the failure attached to the selected row, so Retry import cannot target the other bank.
    const first = screen.getByRole("group", { name: "Plaid connection 1" });
    expect(within(first).getByRole("button", { name: "Import again" })).toBeEnabled();
    await user.click(within(second).getByRole("button", { name: "Retry import" }));
    expect(await screen.findByText("Imported 12 transactions.")).toBeInTheDocument();
    expect(api.importBankTransactions).toHaveBeenNthCalledWith(1, "restored-second");
    expect(api.importBankTransactions).toHaveBeenNthCalledWith(2, "restored-second");
    expect(api.createBankLinkToken).not.toHaveBeenCalled();
    expect(api.exchangeBankToken).not.toHaveBeenCalled();
  });
});


describe("indexing progress after bank import", () => {
  it.each([12, 0])("rechecks progress when importing %i new transactions", async (imported) => {
    api.getBankConnections.mockResolvedValue([restoredConnections[0]]);
    api.getIndexingStatus.mockResolvedValue({ total: 331, indexed: 331, remaining: 0 });
    api.importBankTransactions.mockResolvedValue({ imported });
    const user = userEvent.setup();
    render(<App />);
    await screen.findByText("All 331 transactions ready for search");
    const connection = await screen.findByRole("group", { name: "Plaid connection 1" });
    const calls = api.getIndexingStatus.mock.calls.length;

    // Zero new rows can still mean corrected or removed transactions; it must refresh progress too.
    api.getIndexingStatus.mockResolvedValue({ total: 331, indexed: 172, remaining: 159 });
    await user.click(within(connection).getByRole("button", { name: "Import again" }));
    expect(await screen.findByText("Preparing search: 172 of 331 transactions ready, 159 remaining.")).toBeInTheDocument();
    expect(api.getIndexingStatus).toHaveBeenCalledTimes(calls + 1);
    expect(api.importBankTransactions).toHaveBeenCalledExactlyOnceWith("restored-first");
  });
});
