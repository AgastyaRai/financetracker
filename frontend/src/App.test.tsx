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

  it("shows budget validation beside the budget form", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(screen.getByRole("button", { name: "Save budget" }));

    const budgetCard = screen.getByRole("heading", { name: "Budgets" }).closest(".card");
    expect(budgetCard).not.toBeNull();
    expect(within(budgetCard as HTMLElement).getByText("Budget category is required")).toBeInTheDocument();
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
