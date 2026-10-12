import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";

const apiMocks = vi.hoisted(() => ({
  addTransaction: vi.fn(),
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
  vi.clearAllMocks();
  apiMocks.addTransaction.mockResolvedValue(undefined);
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
