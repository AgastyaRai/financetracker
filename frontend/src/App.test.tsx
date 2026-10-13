import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import type { Budget, SemanticSearchResult, Transaction } from "./types";

const apiMocks = vi.hoisted(() => ({
  addTransaction: vi.fn(),
  updateTransaction: vi.fn(),
  revertTransaction: vi.fn(),
  getBudgetProgress: vi.fn(),
  getBudgets: vi.fn(),
  getTransactions: vi.fn(),
  loginUser: vi.fn(),
  registerUser: vi.fn(),
  semanticSearchTransactions: vi.fn(),
  setUnauthorizedCallback: vi.fn(),
  upsertBudget: vi.fn(),
}));

vi.mock("./api", () => apiMocks);

function tokenWithExpiry(expiryTime: number) {
  const payload = btoa(JSON.stringify({ exp: Math.floor(expiryTime / 1000) }));
  return `header.${payload}.signature`;
}

beforeEach(() => {
  vi.resetAllMocks();
  apiMocks.addTransaction.mockResolvedValue(undefined);
  apiMocks.updateTransaction.mockResolvedValue(undefined);
  apiMocks.getBudgetProgress.mockResolvedValue([]);
  apiMocks.getBudgets.mockResolvedValue([]);
  apiMocks.getTransactions.mockResolvedValue([]);
  apiMocks.loginUser.mockReset();
  apiMocks.registerUser.mockResolvedValue(undefined);
  apiMocks.semanticSearchTransactions.mockResolvedValue({ transactions: [], summary: null });
  apiMocks.setUnauthorizedCallback.mockReset();
  apiMocks.upsertBudget.mockResolvedValue(undefined);
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("authentication screen", () => {
  it("shows the login fields by default", () => {
    render(<App />);

    expect(screen.getByRole("heading", { name: "FinanceTracker" })).toBeInTheDocument();
    expect(screen.getByLabelText("Username or Email")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Create account" })).not.toBeInTheDocument();
  });

  it("switches to the registration fields", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(screen.getByRole("button", { name: "Register" }));

    expect(screen.getByLabelText("Username")).toBeInTheDocument();
    expect(screen.getByLabelText("Email")).toHaveAttribute("type", "email");
    expect(screen.getByRole("button", { name: "Create account" })).toBeInTheDocument();
  });

  it("submits the login form with Enter", async () => {
    const user = userEvent.setup();
    const token = tokenWithExpiry(Date.now() + 60_000);
    apiMocks.loginUser.mockResolvedValue({ access_token: token, user_id: "test-user" });
    render(<App />);

    await user.type(screen.getByLabelText("Username or Email"), "test-user");
    await user.type(screen.getByLabelText("Password"), "password{Enter}");

    await waitFor(() => {
      expect(apiMocks.loginUser).toHaveBeenCalledWith({
        identifier: "test-user",
        password: "password",
      });
    });
  });

  it("rejects a malformed stored token before showing the dashboard", () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    localStorage.setItem("access_token", "not-a-jwt");
    localStorage.setItem("user_id", "legacy-user");

    render(<App />);

    expect(screen.getByRole("heading", { name: "FinanceTracker" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Dashboard" })).not.toBeInTheDocument();
    expect(localStorage.getItem("access_token")).toBeNull();
    expect(localStorage.getItem("user_id")).toBeNull();
  });

  it("keeps the session-expired message visible after automatic logout", () => {
    vi.useFakeTimers();
    const now = new Date("2026-07-24T12:00:00Z");
    vi.setSystemTime(now);
    localStorage.setItem("access_token", tokenWithExpiry(now.getTime() + 60_000));

    render(<App />);

    act(() => {
      vi.advanceTimersByTime(30_000);
    });

    expect(screen.getByRole("heading", { name: "FinanceTracker" })).toBeInTheDocument();
    expect(screen.getByText("Session expired. Please log in again.")).toBeInTheDocument();
  });

  it("does not persist the user ID after login", async () => {
    const user = userEvent.setup();
    const token = tokenWithExpiry(Date.now() + 60_000);
    apiMocks.loginUser.mockResolvedValue({ access_token: token, user_id: "test-user" });
    render(<App />);

    await user.type(screen.getByLabelText("Username or Email"), "test-user");
    await user.type(screen.getByLabelText("Password"), "password");
    await user.click(screen.getAllByRole("button", { name: "Login" })[1]);

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Dashboard" })).toBeInTheDocument();
    });
    expect(localStorage.getItem("access_token")).toBe(token);
    expect(localStorage.getItem("user_id")).toBeNull();
  });

  it("rejects a malformed token returned from login", async () => {
    const user = userEvent.setup();
    vi.spyOn(console, "error").mockImplementation(() => {});
    apiMocks.loginUser.mockResolvedValue({ access_token: "not-a-jwt", user_id: "test-user" });
    render(<App />);

    await user.type(screen.getByLabelText("Username or Email"), "test-user");
    await user.type(screen.getByLabelText("Password"), "password");
    await user.click(screen.getAllByRole("button", { name: "Login" })[1]);

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "FinanceTracker" })).toBeInTheDocument();
    });
    expect(screen.queryByRole("heading", { name: "Dashboard" })).not.toBeInTheDocument();
    expect(localStorage.getItem("access_token")).toBeNull();
  });
});

describe("session expiry after suspension", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-10-12T12:00:00Z"));
    localStorage.setItem("access_token", tokenWithExpiry(Date.now() + 86_400_000));
  });

  it.each(["focus", "pageshow", "visibilitychange"])("expires on %s without waiting for a suspended timer", (event) => {
    render(<App />);
    // Moving the wall clock leaves the timer pending, as happens during suspension.
    vi.setSystemTime(Date.now() + 86_400_000);
    if (event === "visibilitychange") vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
    act(() => { (event === "visibilitychange" ? document : window).dispatchEvent(new Event(event)); });
    expect(screen.queryByRole("heading", { name: "Dashboard" })).not.toBeInTheDocument();
    expect(screen.getByText("Session expired. Please log in again.")).toBeInTheDocument();
    expect(localStorage.getItem("access_token")).toBeNull();
  });

  it("preserves valid sessions on resume and keeps their original deadline", () => {
    render(<App />);
    vi.setSystemTime(Date.now() + 43_200_000);
    act(() => { window.dispatchEvent(new Event("focus")); });
    expect(screen.getByRole("heading", { name: "Dashboard" })).toBeInTheDocument();
    act(() => { vi.advanceTimersByTime(43_170_000); });
    expect(screen.getByText("Session expired. Please log in again.")).toBeInTheDocument();
  });

  it("rechecks the wall clock when a timer fires after a backward clock change", () => {
    localStorage.setItem("access_token", tokenWithExpiry(Date.now() + 60_000));
    render(<App />);
    vi.setSystemTime(Date.now() - 60_000);
    act(() => { vi.advanceTimersByTime(30_000); });
    expect(screen.getByRole("heading", { name: "Dashboard" })).toBeInTheDocument();
    act(() => { vi.advanceTimersByTime(60_000); });
    expect(screen.getByText("Session expired. Please log in again.")).toBeInTheDocument();
  });

  it("clears the old account on resume without deleting a replacement session", () => {
    render(<App />);
    const replacement = tokenWithExpiry(Date.now() + 172_800_000);
    // A suspended tab can regain focus before the queued storage event arrives.
    localStorage.setItem("access_token", replacement);
    act(() => { window.dispatchEvent(new Event("focus")); });
    expect(screen.queryByRole("heading", { name: "Dashboard" })).not.toBeInTheDocument();
    expect(localStorage.getItem("access_token")).toBe(replacement);
  });

  it("rejects an expired session on reload before requesting account data", () => {
    localStorage.setItem("access_token", tokenWithExpiry(Date.now() - 1_000));
    render(<App />);
    expect(screen.queryByRole("heading", { name: "Dashboard" })).not.toBeInTheDocument();
    expect(localStorage.getItem("access_token")).toBeNull();
    expect(apiMocks.getTransactions).not.toHaveBeenCalled();
  });

  it("removes resume checks when unmounted", () => {
    const { unmount } = render(<App />);
    unmount();
    vi.setSystemTime(Date.now() + 86_400_000);
    const replacement = tokenWithExpiry(Date.now() + 60_000);
    localStorage.setItem("access_token", replacement);
    act(() => {
      window.dispatchEvent(new Event("focus"));
      window.dispatchEvent(new Event("pageshow"));
      document.dispatchEvent(new Event("visibilitychange"));
    });
    expect(localStorage.getItem("access_token")).toBe(replacement);
  });

  it("rechecks expiry for a session established by login", async () => {
    localStorage.removeItem("access_token");
    apiMocks.loginUser.mockResolvedValue({ access_token: tokenWithExpiry(Date.now() + 60_000), user_id: "user" });
    render(<App />);
    fireEvent.change(screen.getByLabelText("Username or Email"), { target: { value: "user" } });
    fireEvent.change(screen.getByLabelText("Password"), { target: { value: "password" } });
    await act(async () => { fireEvent.click(screen.getAllByRole("button", { name: "Login" })[1]); });
    expect(screen.getByRole("heading", { name: "Dashboard" })).toBeInTheDocument();
    vi.setSystemTime(Date.now() + 60_000);
    act(() => { window.dispatchEvent(new Event("focus")); });
    expect(screen.getByText("Session expired. Please log in again.")).toBeInTheDocument();
  });

  it("does not expire early when a clock adjustment puts the deadline beyond the timer limit", () => {
    render(<App />);
    vi.setSystemTime(Date.now() - 30 * 86_400_000);
    act(() => { window.dispatchEvent(new Event("focus")); });
    act(() => { vi.advanceTimersByTime(2_147_483_647); });
    expect(screen.getByRole("heading", { name: "Dashboard" })).toBeInTheDocument();
    act(() => { vi.advanceTimersByTime(31 * 86_400_000 - 30_000 - 2_147_483_647); });
    expect(screen.getByText("Session expired. Please log in again.")).toBeInTheDocument();
  });
});

describe("dashboard correctness", () => {
  beforeEach(() => {
    localStorage.setItem("access_token", tokenWithExpiry(Date.now() + 60_000));
  });

  it("clears the client session and returns to login when logging out", async () => {
    const user = userEvent.setup();
    render(<App />);

    expect(screen.getByRole("heading", { name: "Dashboard" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Logout" }));

    expect(screen.getByRole("heading", { name: "FinanceTracker" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Dashboard" })).not.toBeInTheDocument();
    expect(localStorage.getItem("access_token")).toBeNull();
  });

  it("shows budget validation beside the budget form", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(screen.getByRole("button", { name: "Save budget" }));

    const budgetCard = screen.getByRole("heading", { name: "Budgets" }).closest(".card");
    expect(budgetCard).not.toBeNull();
    expect(within(budgetCard as HTMLElement).getByRole("alert")).toHaveTextContent("Budget category is required");
  });

  it("uses labelled numeric amount fields", () => {
    render(<App />);

    const transactionCard = screen.getByRole("heading", { name: "Add transaction" }).closest(".card");
    const budgetCard = screen.getByRole("heading", { name: "Budgets" }).closest(".card");
    expect(transactionCard).not.toBeNull();
    expect(budgetCard).not.toBeNull();

    expect(within(transactionCard as HTMLElement).getByLabelText("Amount")).toHaveAttribute("type", "number");
    expect(within(budgetCard as HTMLElement).getByLabelText("Amount")).toHaveAttribute("type", "number");
  });

  it("submits a transaction with Enter", async () => {
    const user = userEvent.setup();
    render(<App />);

    const transactionCard = screen.getByRole("heading", { name: "Add transaction" }).closest(".card");
    expect(transactionCard).not.toBeNull();
    await user.type(
      within(transactionCard as HTMLElement).getByLabelText("Description (optional)"),
      "Coffee{Enter}",
    );

    await waitFor(() => {
      expect(apiMocks.addTransaction).toHaveBeenCalledWith(
        expect.objectContaining({ amount: "12.34", description: "Coffee" }),
      );
    });
  });

  it("prevents duplicate transaction submissions while the request is pending", async () => {
    const user = userEvent.setup();
    let finishRequest!: () => void;
    apiMocks.addTransaction.mockImplementation(
      () => new Promise<void>((resolve) => { finishRequest = resolve; }),
    );
    render(<App />);

    const addButton = screen.getByRole("button", { name: "Add" });
    await user.click(addButton);

    expect(addButton).toBeDisabled();
    expect(addButton).toHaveTextContent("Adding…");
    await user.click(addButton);
    expect(apiMocks.addTransaction).toHaveBeenCalledTimes(1);

    finishRequest();
    await waitFor(() => expect(addButton).toBeEnabled());
  });

  it("styles a successful budget save as success", async () => {
    const user = userEvent.setup();
    render(<App />);

    const budgetCard = screen.getByRole("heading", { name: "Budgets" }).closest(".card");
    expect(budgetCard).not.toBeNull();
    const categoryInput = within(budgetCard as HTMLElement).getByLabelText("Category");
    await user.type(categoryInput, "Food");
    await user.click(within(budgetCard as HTMLElement).getByRole("button", { name: "Save budget" }));

    const status = await within(budgetCard as HTMLElement).findByText("Budget saved.");
    expect(status).toHaveClass("success");
    expect(status).toHaveAttribute("role", "status");
  });

  it("reports a successful transaction separately from a failed dashboard refresh", async () => {
    const user = userEvent.setup();
    render(<App />);
    await waitFor(() => expect(apiMocks.getTransactions).toHaveBeenCalledTimes(1));
    apiMocks.getTransactions.mockRejectedValueOnce(new Error("offline"));

    await user.click(screen.getByRole("button", { name: "Add" }));

    expect(await screen.findByRole("status")).toHaveTextContent("Transaction added successfully.");
    const refreshWarning = "Transaction was saved, but the dashboard could not refresh: offline";
    expect(await screen.findByRole("alert")).toHaveTextContent(refreshWarning);

    await user.click(screen.getByRole("button", { name: "Add" }));
    await waitFor(() => expect(apiMocks.addTransaction).toHaveBeenCalledTimes(2));
    expect(screen.queryByText(refreshWarning)).not.toBeInTheDocument();
  });

  it("reports a successful budget save clearly when its refresh fails", async () => {
    const user = userEvent.setup();
    render(<App />);
    await waitFor(() => expect(apiMocks.getBudgets).toHaveBeenCalledTimes(1));
    apiMocks.getBudgets.mockRejectedValueOnce(new Error("offline"));

    const budgetCard = screen.getByRole("heading", { name: "Budgets" }).closest(".card");
    expect(budgetCard).not.toBeNull();
    await user.type(within(budgetCard as HTMLElement).getByLabelText("Category"), "Food");
    await user.click(within(budgetCard as HTMLElement).getByRole("button", { name: "Save budget" }));

    expect(await within(budgetCard as HTMLElement).findByRole("alert")).toHaveTextContent(
      "Budget was saved, but the dashboard could not refresh: offline",
    );
  });

  it("uses local calendar values for the initial date and month", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-07-01T01:00:00Z"));
    localStorage.setItem("access_token", tokenWithExpiry(Date.now() + 60_000));

    const localNow = new Date();
    const expectedDate = [
      localNow.getFullYear(),
      String(localNow.getMonth() + 1).padStart(2, "0"),
      String(localNow.getDate()).padStart(2, "0"),
    ].join("-");
    const expectedMonth = expectedDate.slice(0, 7);

    render(<App />);

    expect(screen.getByDisplayValue(expectedDate)).toHaveAttribute("type", "date");
    expect(screen.getAllByDisplayValue(expectedMonth)).toHaveLength(2);
  });

  it("labels budget pace by comparing percent used with percent of the month elapsed", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.setSystemTime(new Date(2026, 9, 10, 12));
    localStorage.setItem("access_token", tokenWithExpiry(Date.now() + 60_000));
    apiMocks.getBudgetProgress.mockResolvedValue([
      { category: "On pace", budget_amount: "100.00", spent: "32.00", remaining: "68.00" },
      { category: "Ahead of pace", budget_amount: "100.00", spent: "33.00", remaining: "67.00" },
      { category: "Exceeded", budget_amount: "100.00", spent: "101.00", remaining: "-1.00" },
    ]);

    render(<App />);

    const onPaceCard = (await screen.findByText("On pace")).closest(".budgetProgressItem");
    const aheadCard = screen.getByText("Ahead of pace").closest(".budgetProgressItem");
    const exceededCard = screen.getByText("Exceeded").closest(".budgetProgressItem");
    expect(onPaceCard).not.toBeNull();
    expect(aheadCard).not.toBeNull();
    expect(exceededCard).not.toBeNull();
    expect(within(onPaceCard as HTMLElement).getByText("On track")).toBeInTheDocument();
    expect(within(aheadCard as HTMLElement).getByText("Watch")).toBeInTheDocument();
    expect(within(exceededCard as HTMLElement).getByText("Over budget")).toBeInTheDocument();
  });

  it("summarizes only transactions from the selected month", async () => {
    const now = new Date();
    const selectedMonth = [
      now.getFullYear(),
      String(now.getMonth() + 1).padStart(2, "0"),
    ].join("-");
    const previousDate = new Date(now.getFullYear(), now.getMonth() - 1, 1);
    const previousMonth = [
      previousDate.getFullYear(),
      String(previousDate.getMonth() + 1).padStart(2, "0"),
    ].join("-");
    const selectedMonthLabel = new Intl.DateTimeFormat(undefined, {
      month: "long",
      year: "numeric",
    }).format(new Date(now.getFullYear(), now.getMonth(), 1));
    apiMocks.getTransactions.mockResolvedValue([
      {
        id: "selected-month-income",
        amount: "100.00",
        kind: "Income",
        category: "Pay",
        date: `${selectedMonth}-05`,
        description: null,
      },
      {
        id: "selected-month-expense",
        amount: "25.00",
        kind: "Expense",
        category: "Food",
        date: `${selectedMonth}-10`,
        description: null,
      },
      {
        id: "previous-month-income",
        amount: "1000.00",
        kind: "Income",
        category: "Pay",
        date: `${previousMonth}-05`,
        description: null,
      },
    ]);

    render(<App />);

    const summaryCard = screen.getByRole("heading", { name: "Summary" }).closest(".card");
    expect(summaryCard).not.toBeNull();
    expect(await within(summaryCard as HTMLElement).findByText("$100.00")).toBeInTheDocument();
    expect(within(summaryCard as HTMLElement).getByText("$25.00")).toBeInTheDocument();
    expect(within(summaryCard as HTMLElement).getByText("$75.00")).toBeInTheDocument();
    expect(within(summaryCard as HTMLElement).getByText(selectedMonthLabel)).toBeInTheDocument();
  });

  it("normalizes signed expense amounts in summaries and category bars", async () => {
    const now = new Date();
    const selectedMonth = [
      now.getFullYear(),
      String(now.getMonth() + 1).padStart(2, "0"),
    ].join("-");
    apiMocks.getTransactions.mockResolvedValue([
      {
        id: "signed-expense",
        amount: "-25.00",
        kind: "Expense",
        category: "Food",
        date: `${selectedMonth}-10`,
        description: null,
      },
    ]);

    render(<App />);

    const summaryCard = screen.getByRole("heading", { name: "Summary" }).closest(".card");
    expect(summaryCard).not.toBeNull();
    const expenseTile = (await within(summaryCard as HTMLElement).findByText("Expense")).closest(".metricTile");
    const netTile = within(summaryCard as HTMLElement).getByText("Net").closest(".metricTile");
    expect(expenseTile).not.toBeNull();
    expect(netTile).not.toBeNull();
    expect(within(expenseTile as HTMLElement).getByText("$25.00")).toBeInTheDocument();
    expect(within(netTile as HTMLElement).getByText("-$25.00")).toBeInTheDocument();

    const transactionsCard = screen.getByRole("heading", { name: "Transactions" }).closest(".card");
    expect(transactionsCard).not.toBeNull();
    expect(within(transactionsCard as HTMLElement).getByText("-$25.00")).toBeInTheDocument();

    const categoryChart = await screen.findByRole("img", { name: "Spending by category chart" });
    const categoryBar = categoryChart.querySelector(".categoryBar");
    expect(categoryBar).not.toBeNull();
    expect(Number(categoryBar?.getAttribute("height"))).toBeGreaterThan(0);
  });

  it("shows an empty analytics state instead of a flat chart", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-07-15T12:00:00Z"));
    localStorage.setItem("access_token", tokenWithExpiry(Date.now() + 60_000));

    render(<App />);

    const analyticsCard = screen.getByRole("heading", { name: "Analytics" }).closest(".card");
    expect(analyticsCard).not.toBeNull();
    expect(
      within(analyticsCard as HTMLElement).getByText("No activity for this month yet."),
    ).toBeInTheDocument();
  });
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

function monthValue(date = new Date()) {
  return date.getFullYear() + "-" + String(date.getMonth() + 1).padStart(2, "0");
}

function privateSearchResult(description: string): SemanticSearchResult {
  return {
    transactions: [{
      transaction: {
        id: description, amount: "25.00", kind: "Expense", category: "Food",
        date: monthValue() + "-10", description,
      },
      similarity_score: 0.9,
    }],
    summary: null,
  };
}

async function signInAgain(user: ReturnType<typeof userEvent.setup>) {
  apiMocks.loginUser.mockResolvedValue({
    access_token: tokenWithExpiry(Date.now() + 180_000), user_id: "next-user",
  });
  await user.type(screen.getByLabelText("Username or Email"), "next-user");
  await user.type(screen.getByLabelText("Password"), "password");
  await user.click(screen.getAllByRole("button", { name: "Login" })[1]);
  await screen.findByRole("heading", { name: "Dashboard" });
}

describe("dashboard request lifecycle", () => {
  beforeEach(() => {
    localStorage.setItem("access_token", tokenWithExpiry(Date.now() + 60_000));
  });

  it.each([0, 1])("keeps a valid selection when month field %i is cleared", (index) => {
    render(<App />);
    const monthInput = screen.getAllByLabelText("Month")[index];
    const selectedMonth = (monthInput as HTMLInputElement).value;
    fireEvent.change(monthInput, { target: { value: "" } });
    expect(monthInput).toHaveValue(selectedMonth);
    expect(screen.getByRole("heading", { name: "Dashboard" })).toBeInTheDocument();
  });

  it("keeps the newer month's budgets when an older response arrives last", async () => {
    const oldMonth = monthValue();
    const now = new Date();
    const nextMonth = monthValue(new Date(now.getFullYear(), now.getMonth() + 1, 1));
    const oldRequest = deferred<Budget[]>();
    apiMocks.getBudgets.mockImplementation((month: string) => month === oldMonth + "-01"
      ? oldRequest.promise
      : Promise.resolve([{ month, category: "Current budget", amount: "200.00" }]));
    render(<App />);
    const budgetCard = screen.getByRole("heading", { name: "Budgets" }).closest(".card") as HTMLElement;
    fireEvent.change(within(budgetCard).getByLabelText("Month"), { target: { value: nextMonth } });
    await within(budgetCard).findByText("Current budget");

    await act(async () => {
      oldRequest.resolve([{ month: oldMonth + "-01", category: "Previous budget", amount: "100.00" }]);
    });

    expect(within(budgetCard).getByText("Current budget")).toBeInTheDocument();
    expect(within(budgetCard).queryByText("Previous budget")).not.toBeInTheDocument();
  });

  it("ignores a failed budget request for a month that is no longer selected", async () => {
    const oldMonth = monthValue();
    const now = new Date();
    const nextMonth = monthValue(new Date(now.getFullYear(), now.getMonth() + 1, 1));
    const oldRequest = deferred<Budget[]>();
    apiMocks.getBudgets.mockImplementation((month: string) => month === oldMonth + "-01"
      ? oldRequest.promise
      : Promise.resolve([{ month, category: "Current budget", amount: "200.00" }]));
    render(<App />);
    fireEvent.change(screen.getAllByLabelText("Month")[0], { target: { value: nextMonth } });
    await screen.findByText("Current budget");
    await act(async () => { oldRequest.reject(new Error("old month failed")); });
    expect(screen.queryByText("old month failed")).not.toBeInTheDocument();
    expect(screen.getByText("Current budget")).toBeInTheDocument();
  });

  it("does not let a completed save reset a new month's budget or draft", async () => {
    const user = userEvent.setup();
    const oldMonth = monthValue();
    const now = new Date();
    const nextMonth = monthValue(new Date(now.getFullYear(), now.getMonth() + 1, 1));
    const save = deferred<void>();
    apiMocks.upsertBudget.mockReturnValue(save.promise);
    apiMocks.getBudgets.mockImplementation((month: string) => Promise.resolve([{
      month, category: month === oldMonth + "-01" ? "Previous budget" : "Current budget", amount: "100.00",
    }]));
    render(<App />);
    const budgetCard = screen.getByRole("heading", { name: "Budgets" }).closest(".card") as HTMLElement;
    await within(budgetCard).findByText("Previous budget");
    const categoryInput = within(budgetCard).getByLabelText("Category");
    await user.type(categoryInput, "Food");
    await user.click(within(budgetCard).getByRole("button", { name: "Save budget" }));
    fireEvent.change(within(budgetCard).getByLabelText("Month"), { target: { value: nextMonth } });
    await within(budgetCard).findByText("Current budget");
    await user.clear(categoryInput);
    await user.type(categoryInput, "Next draft");
    await act(async () => { save.resolve(undefined); });
    await waitFor(() => expect(within(budgetCard).getByRole("button", { name: "Save budget" })).toBeEnabled());
    expect(categoryInput).toHaveValue("Next draft");
    expect(within(budgetCard).getByText("Current budget")).toBeInTheDocument();
    expect(apiMocks.getBudgets).toHaveBeenLastCalledWith(nextMonth + "-01");
  });

  it("keeps a replacement search pending when the cleared search finishes", async () => {
    const user = userEvent.setup();
    const firstSearch = deferred<SemanticSearchResult>();
    const nextSearch = deferred<SemanticSearchResult>();
    apiMocks.semanticSearchTransactions.mockReturnValueOnce(firstSearch.promise).mockReturnValueOnce(nextSearch.promise);
    render(<App />);
    const queryInput = screen.getByRole("textbox", { name: "Semantic search query" });
    await user.type(queryInput, "coffee");
    await user.click(screen.getByRole("button", { name: "Search" }));
    await user.click(screen.getByRole("button", { name: "Clear" }));
    expect(screen.getByRole("button", { name: "Search" })).toBeEnabled();
    await user.type(queryInput, "train");
    await user.click(screen.getByRole("button", { name: "Search" }));
    expect(apiMocks.semanticSearchTransactions).toHaveBeenCalledTimes(2);
    await act(async () => { firstSearch.resolve(privateSearchResult("Old search result")); });
    expect(screen.queryByText("Old search result")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Searching..." })).toBeDisabled();
    await act(async () => { nextSearch.resolve(privateSearchResult("Current search result")); });
    expect(await screen.findByText("Current search result")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Search" })).toBeEnabled();
  });

  it("ignores a search failure after Clear", async () => {
    const user = userEvent.setup();
    const search = deferred<SemanticSearchResult>();
    apiMocks.semanticSearchTransactions.mockReturnValue(search.promise);
    render(<App />);
    await user.type(screen.getByRole("textbox", { name: "Semantic search query" }), "coffee");
    await user.click(screen.getByRole("button", { name: "Search" }));
    await user.click(screen.getByRole("button", { name: "Clear" }));
    await act(async () => { search.reject(new Error("cleared search failed")); });
    expect(screen.queryByText("cleared search failed")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Search" })).toBeEnabled();
  });

  it("clears private search results and drafts before the next login", async () => {
    const user = userEvent.setup();
    apiMocks.semanticSearchTransactions.mockResolvedValue({
      ...privateSearchResult("Private previous-account result"), summary: "Private previous-account summary",
    });
    render(<App />);
    await user.type(screen.getByLabelText("Description (optional)"), "Private transaction draft");
    await user.type(screen.getByRole("textbox", { name: "Semantic search query" }), "private query");
    await user.click(screen.getByRole("button", { name: "Search" }));
    await screen.findByText("Private previous-account result");
    await user.click(screen.getByRole("button", { name: "Logout" }));
    await signInAgain(user);
    expect(screen.queryByText("Private previous-account result")).not.toBeInTheDocument();
    expect(screen.queryByText("Private previous-account summary")).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Semantic search query" })).toHaveValue("");
    expect(screen.getByLabelText("Description (optional)")).toHaveValue("");
  });

  it("ignores a previous account's pending search after another login", async () => {
    const user = userEvent.setup();
    const search = deferred<SemanticSearchResult>();
    apiMocks.semanticSearchTransactions.mockReturnValue(search.promise);
    render(<App />);
    await user.type(screen.getByRole("textbox", { name: "Semantic search query" }), "private query");
    await user.click(screen.getByRole("button", { name: "Search" }));
    await user.click(screen.getByRole("button", { name: "Logout" }));
    await signInAgain(user);
    await act(async () => { search.resolve(privateSearchResult("Private late result")); });
    expect(screen.queryByText("Private late result")).not.toBeInTheDocument();
  });

  it("ignores a previous account's transaction response after another login", async () => {
    const user = userEvent.setup();
    const oldRequest = deferred<Transaction[]>();
    const nextRequest = deferred<Transaction[]>();
    apiMocks.getTransactions.mockReturnValueOnce(oldRequest.promise).mockReturnValueOnce(nextRequest.promise);
    render(<App />);
    await user.click(screen.getByRole("button", { name: "Logout" }));
    await signInAgain(user);
    await act(async () => {
      oldRequest.resolve([{ ...privateSearchResult("Private transaction").transactions[0].transaction, amount: "700.00" }]);
    });
    const summaryCard = screen.getByRole("heading", { name: "Summary" }).closest(".card") as HTMLElement;
    const expenseTile = within(summaryCard).getByText("Expense").closest(".metricTile") as HTMLElement;
    expect(expenseTile).toHaveTextContent("$0.00");
    await act(async () => { nextRequest.resolve([]); });
  });

  it("ignores a previous account's write completion after another login", async () => {
    const user = userEvent.setup();
    const save = deferred<void>();
    apiMocks.addTransaction.mockReturnValue(save.promise);
    render(<App />);
    await user.click(screen.getByRole("button", { name: "Add" }));
    await user.click(screen.getByRole("button", { name: "Logout" }));
    await signInAgain(user);
    await user.type(screen.getByLabelText("Description (optional)"), "New account draft");
    await act(async () => { save.resolve(undefined); });
    expect(screen.queryByText("Transaction added successfully.")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Description (optional)")).toHaveValue("New account draft");
  });

  it("preserves a new description typed while the previous transaction is saving", async () => {
    const user = userEvent.setup();
    const save = deferred<void>();
    apiMocks.addTransaction.mockReturnValue(save.promise);
    render(<App />);
    const description = screen.getByLabelText("Description (optional)");
    await user.type(description, "First draft");
    await user.click(screen.getByRole("button", { name: "Add" }));
    await user.clear(description);
    await user.type(description, "Next draft");
    await act(async () => { save.resolve(undefined); });
    await waitFor(() => expect(screen.getByRole("button", { name: "Add" })).toBeEnabled());
    expect(description).toHaveValue("Next draft");
  });

  it("cleans up the logout timer and unauthorized callback when unmounted", () => {
    vi.useFakeTimers();
    const now = new Date("2026-10-11T12:00:00Z");
    vi.setSystemTime(now);
    localStorage.setItem("access_token", tokenWithExpiry(now.getTime() + 60_000));
    const { unmount } = render(<App />);
    unmount();
    const nextToken = tokenWithExpiry(now.getTime() + 180_000);
    localStorage.setItem("access_token", nextToken);
    act(() => { vi.advanceTimersByTime(30_000); });
    expect(localStorage.getItem("access_token")).toBe(nextToken);
    expect(apiMocks.setUnauthorizedCallback).toHaveBeenLastCalledWith(null);
  });

  it("clears this account's view when another tab replaces the session", async () => {
    const user = userEvent.setup();
    render(<App />);
    await user.type(screen.getByLabelText("Description (optional)"), "Private draft");
    const oldToken = localStorage.getItem("access_token");
    const nextToken = tokenWithExpiry(Date.now() + 180_000);
    localStorage.setItem("access_token", nextToken);
    act(() => {
      window.dispatchEvent(new StorageEvent("storage", {
        key: "access_token", oldValue: oldToken, newValue: nextToken, storageArea: localStorage,
      }));
    });
    expect(screen.getByRole("heading", { name: "FinanceTracker" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Dashboard" })).not.toBeInTheDocument();
    expect(localStorage.getItem("access_token")).toBe(nextToken);
  });
});

describe("authentication request lifecycle", () => {
  it("does not establish a session after the login screen unmounts", async () => {
    const user = userEvent.setup();
    const login = deferred<{ access_token: string; user_id: string }>();
    apiMocks.loginUser.mockReturnValue(login.promise);
    const { unmount } = render(<App />);
    await user.type(screen.getByLabelText("Username or Email"), "user");
    await user.type(screen.getByLabelText("Password"), "password");
    await user.click(screen.getAllByRole("button", { name: "Login" })[1]);
    unmount();
    await act(async () => {
      login.resolve({ access_token: tokenWithExpiry(Date.now() + 180_000), user_id: "user" });
    });
    expect(localStorage.getItem("access_token")).toBeNull();
  });

  it("does not replace a newer cross-tab session with a pending login", async () => {
    const user = userEvent.setup();
    const login = deferred<{ access_token: string; user_id: string }>();
    apiMocks.loginUser.mockReturnValue(login.promise);
    render(<App />);
    await user.type(screen.getByLabelText("Username or Email"), "user");
    await user.type(screen.getByLabelText("Password"), "password");
    await user.click(screen.getAllByRole("button", { name: "Login" })[1]);
    const nextToken = tokenWithExpiry(Date.now() + 240_000);
    localStorage.setItem("access_token", nextToken);
    act(() => {
      window.dispatchEvent(new StorageEvent("storage", {
        key: "access_token", oldValue: null, newValue: nextToken, storageArea: localStorage,
      }));
    });
    await act(async () => {
      login.resolve({ access_token: tokenWithExpiry(Date.now() + 180_000), user_id: "user" });
    });
    expect(localStorage.getItem("access_token")).toBe(nextToken);
    expect(screen.queryByRole("heading", { name: "Dashboard" })).not.toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Login" })[1]).toBeEnabled();
  });
});

describe("responsive chart data", () => {
  const fullCategory = "Groceries and household supplies";

  beforeEach(() => {
    localStorage.setItem("access_token", tokenWithExpiry(Date.now() + 180_000));
    apiMocks.getTransactions.mockResolvedValue([
      { id: "chart-income", amount: "100.00", kind: "Income", category: "Pay", date: monthValue() + "-05", description: null },
      { id: "chart-expense", amount: "-25.00", kind: "Expense", category: fullCategory, date: monthValue() + "-10", description: null },
    ]);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("uses the available width when chart containers resize", async () => {
    const observers: Array<{ callback: ResizeObserverCallback; target: Element; observer: ResizeObserver }> = [];
    vi.stubGlobal("ResizeObserver", class {
      callback: ResizeObserverCallback;
      constructor(callback: ResizeObserverCallback) { this.callback = callback; }
      observe(target: Element) {
        observers.push({ callback: this.callback, target, observer: this as unknown as ResizeObserver });
      }
      disconnect() {}
      unobserve() {}
    });
    render(<App />);
    const netChart = await screen.findByRole("img", { name: "Cumulative net chart" });
    const categoryChart = await screen.findByRole("img", { name: "Spending by category chart" });
    await act(async () => {
      for (const { callback, target, observer } of observers) {
        callback([{ target, contentRect: { width: 280 } } as ResizeObserverEntry], observer);
      }
    });
    expect(netChart).toHaveAttribute("viewBox", "0 0 280 190");
    expect(categoryChart).toHaveAttribute("viewBox", "0 0 280 210");
  });

  it("describes actual balances and full category totals for assistive technology", async () => {
    render(<App />);
    const netChart = await screen.findByRole("img", { name: "Cumulative net chart" });
    const categoryChart = await screen.findByRole("img", { name: "Spending by category chart" });
    expect(netChart).toHaveAccessibleDescription(expect.stringContaining("$75.00"));
    expect(netChart).toHaveAccessibleDescription(expect.stringContaining("$100.00"));
    expect(categoryChart).toHaveAccessibleDescription(fullCategory + ": $25.00.");
  });

  it("offers exact category totals without shortening category names", async () => {
    const user = userEvent.setup();
    render(<App />);
    await screen.findByRole("img", { name: "Spending by category chart" });
    await user.click(screen.getByText("Category totals", { exact: true }));
    const table = screen.getByRole("table", { name: "Spending category totals" });
    expect(within(table).getByText(fullCategory)).toBeInTheDocument();
    expect(within(table).getByText("$25.00")).toBeInTheDocument();
  });

  it("offers daily balances with the same signed values shown in the chart", async () => {
    const user = userEvent.setup();
    apiMocks.getTransactions.mockResolvedValue([
      { id: "only-expense", amount: "25.00", kind: "Expense", category: fullCategory, date: monthValue() + "-01", description: null },
    ]);
    render(<App />);
    await screen.findByRole("img", { name: "Cumulative net chart" });
    await user.click(screen.getByText("Daily balances", { exact: true }));
    const table = screen.getByRole("table", { name: "Daily cumulative net balances" });
    const firstDay = within(table).getByText("Day 1").closest("tr") as HTMLElement;
    expect(within(firstDay).getByText("-$25.00")).toBeInTheDocument();
  });
});


describe("transaction editing", () => {
  function originalTransaction(): Transaction {
    return {
      id: "edit-coffee", amount: "-25.00", kind: "Expense", category: "Food",
      date: monthValue() + "-10", description: "Coffee",
    };
  }

  function editor() {
    return screen.getByRole("form", { name: "Edit transaction" }).closest("tr") as HTMLElement;
  }

  async function openEditor(user: ReturnType<typeof userEvent.setup>) {
    await user.click(await screen.findByRole("button", { name: /Edit transaction: Coffee/ }));
    return within(editor());
  }

  beforeEach(() => {
    localStorage.setItem("access_token", tokenWithExpiry(Date.now() + 180_000));
    apiMocks.getTransactions.mockResolvedValue([originalTransaction()]);
  });

  it("turns only the selected transaction row into editable cells", async () => {
    const user = userEvent.setup();
    apiMocks.getTransactions.mockResolvedValue([
      originalTransaction(), { ...originalTransaction(), id: "other-row", description: "Train" },
    ]);
    render(<App />);
    await openEditor(user);
    const form = screen.getByRole("form", { name: "Edit transaction" });
    const row = form.closest("tr");
    expect(row).not.toBeNull();
    expect(screen.queryByRole("heading", { name: "Edit transaction" })).not.toBeInTheDocument();
    expect(within(row as HTMLElement).getByLabelText("Amount").closest("td")).toBeInTheDocument();
    expect(within(row as HTMLElement).getByLabelText("Date").closest("td")).toBeInTheDocument();
    const otherRow = screen.getByText("Train").closest("tr") as HTMLElement;
    expect(within(otherRow).queryByRole("textbox")).not.toBeInTheDocument();
    expect(within(otherRow).getByRole("button", { name: /Edit transaction: Train/ })).toBeInTheDocument();
  });

  it("submits inline fields with Enter and cancels with Escape", async () => {
    const user = userEvent.setup();
    render(<App />);
    await openEditor(user);
    const row = screen.getByRole("form", { name: "Edit transaction" }).closest("tr") as HTMLElement;
    await user.clear(within(row).getByLabelText("Description (optional)"));
    await user.type(within(row).getByLabelText("Description (optional)"), "Inline coffee{Enter}");
    await waitFor(() => expect(apiMocks.updateTransaction).toHaveBeenCalledWith("edit-coffee", expect.objectContaining({ description: "Inline coffee" })));
    await screen.findByText("Transaction updated successfully.");
    await openEditor(user);
    const nextRow = screen.getByRole("form", { name: "Edit transaction" }).closest("tr") as HTMLElement;
    await user.type(within(nextRow).getByLabelText("Description (optional)"), "cancelled{Escape}");
    expect(screen.queryByRole("form", { name: "Edit transaction" })).not.toBeInTheDocument();
    expect(apiMocks.updateTransaction).toHaveBeenCalledTimes(1);
  });

  it("prefills the existing fields and cancels without writing", async () => {
    const user = userEvent.setup();
    render(<App />);
    const form = await openEditor(user);
    expect(form.getByLabelText("Amount")).toHaveValue(25);
    expect(form.getByLabelText("Amount")).toHaveFocus();
    expect(form.getByLabelText("Kind")).toHaveValue("Expense");
    expect(form.getByLabelText("Date")).toHaveValue(monthValue() + "-10");
    expect(form.getByLabelText("Category (optional)")).toHaveValue("Food");
    expect(form.getByLabelText("Description (optional)")).toHaveValue("Coffee");
    await user.clear(form.getByLabelText("Description (optional)"));
    await user.type(form.getByLabelText("Description (optional)"), "Unsaved draft");
    await user.click(form.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("form", { name: "Edit transaction" })).not.toBeInTheDocument();
    expect(apiMocks.updateTransaction).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: /Edit transaction: Coffee/ })).toHaveFocus();
    expect(screen.getByText("Coffee")).toBeInTheDocument();
  });

  it("saves all fields and refreshes the selected-month dashboard", async () => {
    const user = userEvent.setup();
    render(<App />);
    const form = await openEditor(user);
    await user.clear(form.getByLabelText("Amount"));
    await user.type(form.getByLabelText("Amount"), "40.50");
    await user.selectOptions(form.getByLabelText("Kind"), "Income");
    fireEvent.change(form.getByLabelText("Date"), { target: { value: monthValue() + "-12" } });
    await user.clear(form.getByLabelText("Category (optional)"));
    await user.type(form.getByLabelText("Category (optional)"), " Refund ");
    await user.clear(form.getByLabelText("Description (optional)"));
    await user.type(form.getByLabelText("Description (optional)"), " Returned coffee ");
    const updated = {
      ...originalTransaction(), amount: "40.50", kind: "Income" as const,
      category: "Refund", date: monthValue() + "-12", description: "Returned coffee",
    };
    apiMocks.getTransactions.mockResolvedValue([updated]);
    await user.click(form.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(apiMocks.updateTransaction).toHaveBeenCalledWith("edit-coffee", {
      amount: "40.5", kind: "Income", category: "Refund",
      date: monthValue() + "-12", description: "Returned coffee",
    }));
    expect(await screen.findByText("Transaction updated successfully.")).toBeInTheDocument();
    expect(screen.queryByRole("form", { name: "Edit transaction" })).not.toBeInTheDocument();
    await waitFor(() => expect(apiMocks.getTransactions).toHaveBeenCalledTimes(2));
    expect(apiMocks.getBudgetProgress).toHaveBeenLastCalledWith(monthValue() + "-01");
    const summary = screen.getByRole("heading", { name: "Summary" }).closest(".card") as HTMLElement;
    expect(within(summary).getByText("Income").closest(".metricTile")).toHaveTextContent("$40.50");
    expect(within(summary).getByText("Expense").closest(".metricTile")).toHaveTextContent("$0.00");
  });

  it("saves empty optional fields as null", async () => {
    const user = userEvent.setup();
    apiMocks.getTransactions.mockResolvedValue([{ ...originalTransaction(), category: null, description: null }]);
    render(<App />);
    await user.click(await screen.findByRole("button", { name: /Edit transaction: Expense/ }));
    const form = within(editor());
    expect(form.getByLabelText("Category (optional)")).toHaveValue("");
    expect(form.getByLabelText("Description (optional)")).toHaveValue("");
    await user.type(form.getByLabelText("Category (optional)"), "   ");
    await user.click(form.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(apiMocks.updateTransaction).toHaveBeenCalledWith("edit-coffee", expect.objectContaining({
      amount: "25.00", category: null, description: null,
    })));
  });

  it("rejects a zero amount without sending a request", async () => {
    const user = userEvent.setup();
    render(<App />);
    const form = await openEditor(user);
    fireEvent.change(form.getByLabelText("Amount"), { target: { value: "0" } });
    fireEvent.submit(screen.getByRole("form", { name: "Edit transaction" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Amount must be greater than zero");
    expect(apiMocks.updateTransaction).not.toHaveBeenCalled();
  });

  it("keeps the draft after an API failure so saving can be retried", async () => {
    const user = userEvent.setup();
    apiMocks.updateTransaction.mockRejectedValueOnce(new Error("Transaction not found"));
    render(<App />);
    const form = await openEditor(user);
    await user.clear(form.getByLabelText("Description (optional)"));
    await user.type(form.getByLabelText("Description (optional)"), "Corrected coffee");
    await user.click(form.getByRole("button", { name: "Save changes" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Transaction not found");
    expect(form.getByLabelText("Description (optional)")).toHaveValue("Corrected coffee");
    expect(apiMocks.getTransactions).toHaveBeenCalledTimes(1);
    await user.click(form.getByRole("button", { name: "Save changes" }));
    await screen.findByText("Transaction updated successfully.");
    expect(apiMocks.updateTransaction).toHaveBeenCalledTimes(2);
  });

  it("prevents duplicate saves and draft changes while a write is pending", async () => {
    const user = userEvent.setup();
    const save = deferred<void>();
    apiMocks.updateTransaction.mockReturnValue(save.promise);
    render(<App />);
    const form = await openEditor(user);
    await user.click(form.getByRole("button", { name: "Save changes" }));
    expect(form.getByRole("button", { name: "Saving…" })).toBeDisabled();
    expect(form.getByRole("button", { name: "Cancel" })).toBeDisabled();
    expect(form.getByLabelText("Amount")).toBeDisabled();
    fireEvent.submit(screen.getByRole("form", { name: "Edit transaction" }));
    expect(apiMocks.updateTransaction).toHaveBeenCalledTimes(1);
    await act(async () => { save.resolve(undefined); });
    await screen.findByText("Transaction updated successfully.");
  });

  it("distinguishes a completed edit from a failed refresh and retains the saved row", async () => {
    const user = userEvent.setup();
    render(<App />);
    const form = await openEditor(user);
    await user.clear(form.getByLabelText("Description (optional)"));
    await user.type(form.getByLabelText("Description (optional)"), "Saved coffee");
    apiMocks.getTransactions.mockRejectedValueOnce(new Error("offline"));
    await user.click(form.getByRole("button", { name: "Save changes" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Transaction was updated, but the dashboard could not refresh: offline");
    expect(screen.queryByRole("form", { name: "Edit transaction" })).not.toBeInTheDocument();
    expect(await screen.findByText("Saved coffee")).toBeInTheDocument();
    expect(apiMocks.updateTransaction).toHaveBeenCalledTimes(1);
  });

  it("refreshes the accepted search rather than a query draft and removes its stale AI summary", async () => {
    const user = userEvent.setup();
    const refreshedSearch = deferred<SemanticSearchResult>();
    apiMocks.semanticSearchTransactions.mockResolvedValueOnce({
      transactions: [{ transaction: originalTransaction(), similarity_score: 0.9 }], summary: "Old AI summary",
    }).mockReturnValueOnce(refreshedSearch.promise);
    render(<App />);
    await screen.findByText("Coffee");
    await user.type(screen.getByRole("textbox", { name: "Semantic search query" }), "coffee");
    await user.click(screen.getByLabelText("Summarize with AI"));
    await user.click(screen.getByRole("button", { name: "Search" }));
    await screen.findByText("Old AI summary");
    const form = await openEditor(user);
    await user.clear(screen.getByRole("textbox", { name: "Semantic search query" }));
    await user.type(screen.getByRole("textbox", { name: "Semantic search query" }), "next query draft");
    await user.click(form.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(apiMocks.semanticSearchTransactions).toHaveBeenCalledTimes(2));
    expect(apiMocks.semanticSearchTransactions).toHaveBeenLastCalledWith({ query: "coffee", limit: 10, summary: true });
    expect(screen.queryByText("Old AI summary")).not.toBeInTheDocument();
    await act(async () => { refreshedSearch.resolve({ transactions: [], summary: "Updated AI summary" }); });
    expect(await screen.findByText("No matching transactions found.")).toBeInTheDocument();
    expect(screen.getByText("Updated AI summary")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Semantic search query" })).toHaveValue("next query draft");
  });

  it("does not restore a search cleared while saving an edit", async () => {
    const user = userEvent.setup();
    const save = deferred<void>();
    apiMocks.updateTransaction.mockReturnValue(save.promise);
    apiMocks.semanticSearchTransactions.mockResolvedValue({
      transactions: [{ transaction: originalTransaction(), similarity_score: 0.9 }], summary: null,
    });
    render(<App />);
    await screen.findByText("Coffee");
    await user.type(screen.getByRole("textbox", { name: "Semantic search query" }), "coffee");
    await user.click(screen.getByRole("button", { name: "Search" }));
    const form = await openEditor(user);
    await user.click(form.getByRole("button", { name: "Save changes" }));
    await user.click(screen.getByRole("button", { name: "Clear" }));
    await act(async () => { save.resolve(undefined); });
    await screen.findByText("Transaction updated successfully.");
    expect(apiMocks.semanticSearchTransactions).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("textbox", { name: "Semantic search query" })).toHaveValue("");
  });

  it("ignores a search response that started before the edit finished", async () => {
    const user = userEvent.setup();
    const oldSearch = deferred<SemanticSearchResult>();
    apiMocks.semanticSearchTransactions.mockReturnValueOnce(oldSearch.promise).mockResolvedValueOnce({ transactions: [], summary: null });
    render(<App />);
    const form = await openEditor(user);
    await user.type(screen.getByRole("textbox", { name: "Semantic search query" }), "coffee");
    await user.click(screen.getByRole("button", { name: "Search" }));
    await user.click(form.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(apiMocks.semanticSearchTransactions).toHaveBeenCalledTimes(2));
    await act(async () => { oldSearch.resolve({ transactions: [{ transaction: originalTransaction(), similarity_score: 0.9 }], summary: "Stale summary" }); });
    expect(await screen.findByText("No matching transactions found.")).toBeInTheDocument();
    expect(screen.queryByText("Stale summary")).not.toBeInTheDocument();
  });

  it("ignores a failed search refresh after Clear while dashboard reads are still pending", async () => {
    const user = userEvent.setup();
    const transactionsRefresh = deferred<Transaction[]>();
    const searchRefresh = deferred<SemanticSearchResult>();
    apiMocks.semanticSearchTransactions.mockResolvedValueOnce({
      transactions: [{ transaction: originalTransaction(), similarity_score: 0.9 }], summary: null,
    }).mockReturnValueOnce(searchRefresh.promise);
    render(<App />);
    await screen.findByText("Coffee");
    await user.type(screen.getByRole("textbox", { name: "Semantic search query" }), "coffee");
    await user.click(screen.getByRole("button", { name: "Search" }));
    const form = await openEditor(user);
    apiMocks.getTransactions.mockReturnValueOnce(transactionsRefresh.promise);
    await user.click(form.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(apiMocks.semanticSearchTransactions).toHaveBeenCalledTimes(2));
    await act(async () => { searchRefresh.reject(new Error("search refresh failed")); });
    await user.click(screen.getByRole("button", { name: "Clear" }));
    await act(async () => { transactionsRefresh.resolve([originalTransaction()]); });
    await waitFor(() => expect(screen.getByRole("button", { name: /Edit transaction: Coffee/ })).toBeEnabled());
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("retains a saved search-result row when the regular list could not load", async () => {
    const user = userEvent.setup();
    apiMocks.getTransactions.mockRejectedValue(new Error("list offline"));
    apiMocks.semanticSearchTransactions.mockResolvedValueOnce({
      transactions: [{ transaction: originalTransaction(), similarity_score: 0.9 }], summary: null,
    }).mockResolvedValueOnce({ transactions: [], summary: null });
    render(<App />);
    await screen.findByRole("alert");
    await user.type(screen.getByRole("textbox", { name: "Semantic search query" }), "coffee");
    await user.click(screen.getByRole("button", { name: "Search" }));
    const form = await openEditor(user);
    await user.clear(form.getByLabelText("Description (optional)"));
    await user.type(form.getByLabelText("Description (optional)"), "Saved search edit");
    await user.click(form.getByRole("button", { name: "Save changes" }));
    await screen.findByText("No matching transactions found.");
    await user.click(screen.getByRole("button", { name: "Clear" }));
    expect(await screen.findByText("Saved search edit")).toBeInTheDocument();
  });

  it("clears the edit draft and ignores its completion after another login", async () => {
    const user = userEvent.setup();
    const save = deferred<void>();
    apiMocks.updateTransaction.mockReturnValue(save.promise);
    render(<App />);
    const form = await openEditor(user);
    await user.click(form.getByRole("button", { name: "Save changes" }));
    await user.click(screen.getByRole("button", { name: "Logout" }));
    apiMocks.getTransactions.mockResolvedValue([]);
    await signInAgain(user);
    await act(async () => { save.resolve(undefined); });
    expect(screen.queryByRole("form", { name: "Edit transaction" })).not.toBeInTheDocument();
    expect(screen.queryByText("Transaction updated successfully.")).not.toBeInTheDocument();
    expect(apiMocks.getTransactions).toHaveBeenCalledTimes(2);
  });
});


describe("imported transaction revert", () => {
  const edited = {
    id: "imported-coffee", amount: "75.50", kind: "Income" as const,
    category: "Edited", date: monthValue() + "-12", description: "Edited import", can_revert: true,
  };
  const original = { kind: "Expense" as const, category: "Transportation", description: "Original commute" };
  const restored = { ...edited, ...original, can_revert: false };

  beforeEach(() => {
    localStorage.setItem("access_token", tokenWithExpiry(Date.now() + 180_000));
    apiMocks.getTransactions.mockResolvedValue([edited]);
    apiMocks.revertTransaction.mockResolvedValue(original);
  });

  it("offers revert only for eligible rows and preserves amount/date when dashboard refresh fails", async () => {
    const user = userEvent.setup();
    apiMocks.getTransactions.mockResolvedValueOnce([edited, { ...edited, id: "manual", description: "Manual", can_revert: false }])
      .mockRejectedValue(new Error("Refresh unavailable"));
    render(<App />);
    const revert = await screen.findByRole("button", { name: /Revert imported fields: Edited import/ });
    expect(screen.getAllByRole("button", { name: /Revert imported fields:/ })).toHaveLength(1);
    await user.click(revert);
    expect(apiMocks.revertTransaction).toHaveBeenCalledExactlyOnceWith("imported-coffee");
    expect(await screen.findByRole("alert")).toHaveTextContent("reverted, but the dashboard could not refresh");
    const row = screen.getByText("Original commute").closest("tr") as HTMLElement;
    expect(row).toHaveTextContent("$75.50");
    expect(row).toHaveTextContent(edited.date);
    expect(within(row).queryByRole("button", { name: /Revert imported fields:/ })).not.toBeInTheDocument();
  });

  it("refreshes totals and the accepted semantic search after restoring the row", async () => {
    const user = userEvent.setup();
    apiMocks.semanticSearchTransactions.mockResolvedValueOnce({ transactions: [{ transaction: edited, similarity_score: 0.9 }], summary: "Old summary" })
      .mockResolvedValue({ transactions: [{ transaction: restored, similarity_score: 0.8 }], summary: "Restored summary" });
    render(<App />);
    await screen.findByText("Edited import");
    await user.type(screen.getByLabelText("Semantic search query"), "commute");
    await user.click(screen.getByRole("button", { name: "Search" }));
    await screen.findByText("Old summary");
    apiMocks.getTransactions.mockResolvedValue([restored]);
    await user.click(screen.getByRole("button", { name: /Revert imported fields:/ }));
    expect(await screen.findByText("Imported fields reverted successfully.")).toBeInTheDocument();
    await screen.findByText("Restored summary");
    expect(screen.queryByText("Old summary")).not.toBeInTheDocument();
    expect(apiMocks.getBudgetProgress).toHaveBeenLastCalledWith(monthValue() + "-01");
    expect(apiMocks.semanticSearchTransactions).toHaveBeenCalledTimes(2);
    const summary = screen.getByRole("heading", { name: "Summary" }).closest(".card") as HTMLElement;
    expect(within(summary).getByText("Expense").closest(".metricTile")).toHaveTextContent("$75.50");
  });

  it("keeps the row and allows retry when revert fails", async () => {
    const user = userEvent.setup();
    apiMocks.revertTransaction.mockRejectedValueOnce(new Error("Imported transaction not found"));
    render(<App />);
    await user.click(await screen.findByRole("button", { name: /Revert imported fields:/ }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Imported transaction not found");
    expect(screen.getByText("Edited import")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Revert imported fields:/ })).toBeEnabled();
    expect(apiMocks.getTransactions).toHaveBeenCalledTimes(1);
  });

  it("protects unsaved edits and prevents duplicate writes while reverting", async () => {
    const user = userEvent.setup();
    apiMocks.getTransactions.mockResolvedValue([edited, { ...edited, id: "another", description: "Another import" }]);
    const pending = deferred<typeof original>();
    apiMocks.revertTransaction.mockReturnValue(pending.promise);
    render(<App />);
    await user.click(await screen.findByRole("button", { name: /Edit transaction: Another import/ }));
    expect(screen.getByRole("button", { name: /Revert imported fields: Edited import/ })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    const revert = screen.getByRole("button", { name: /Revert imported fields: Edited import/ });
    await user.click(revert);
    expect(revert).toBeDisabled();
    expect(screen.getByRole("button", { name: /Edit transaction: Another import/ })).toBeDisabled();
    await user.click(revert);
    expect(apiMocks.revertTransaction).toHaveBeenCalledTimes(1);
    await act(async () => pending.resolve(original));
  });

  it("preserves focus when the user moves to the search box during a pending revert", async () => {
    const user = userEvent.setup();
    const pending = deferred<typeof original>();
    apiMocks.revertTransaction.mockReturnValue(pending.promise);
    render(<App />);
    await user.click(await screen.findByRole("button", { name: /Revert imported fields:/ }));
    const query = screen.getByLabelText("Semantic search query");
    await user.type(query, "draft search");
    apiMocks.getTransactions.mockResolvedValue([restored]);
    await act(async () => pending.resolve(original));
    await screen.findByText("Imported fields reverted successfully.");
    await screen.findByRole("button", { name: /Edit transaction: Original commute/ });
    expect(query).toHaveFocus();
    expect(query).toHaveValue("draft search");
  });

  it("ignores a pending revert after logout", async () => {
    const user = userEvent.setup();
    const pending = deferred<typeof original>();
    apiMocks.revertTransaction.mockReturnValue(pending.promise);
    render(<App />);
    await user.click(await screen.findByRole("button", { name: /Revert imported fields:/ }));
    await user.click(screen.getByRole("button", { name: "Logout" }));
    await act(async () => pending.resolve(original));
    expect(screen.getByRole("heading", { name: "FinanceTracker" })).toBeInTheDocument();
    expect(screen.queryByText("Imported fields reverted successfully.")).not.toBeInTheDocument();
    expect(apiMocks.getTransactions).toHaveBeenCalledTimes(1);
  });
});
