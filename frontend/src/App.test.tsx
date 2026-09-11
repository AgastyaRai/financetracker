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

  it("submits an Expense as a negative amount with Enter", async () => {
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
        expect.objectContaining({ amount: "-12.34", kind: "Expense", description: "Coffee" }),
      );
    });
  });

  it("submits an Income as a positive amount with Enter", async () => {
    const user = userEvent.setup();
    render(<App />);

    const transactionCard = screen.getByRole("heading", { name: "Add transaction" }).closest(".card");
    expect(transactionCard).not.toBeNull();
    await user.selectOptions(
      within(transactionCard as HTMLElement).getByLabelText("Kind"),
      "Income",
    );
    await user.type(
      within(transactionCard as HTMLElement).getByLabelText("Description (optional)"),
      "Salary{Enter}",
    );

    await waitFor(() => {
      expect(apiMocks.addTransaction).toHaveBeenCalledWith(
        expect.objectContaining({ amount: "12.34", kind: "Income", description: "Salary" }),
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

  it("uses signed amounts for selected-month totals and plots a positive Expense refund as money in", async () => {
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
        id: "selected-month-outflow",
        amount: "-25.00",
        kind: "Income",
        category: "Food",
        date: `${selectedMonth}-10`,
        description: null,
      },
      {
        id: "selected-month-refund",
        amount: "10.00",
        kind: "Expense",
        category: "Food",
        date: `${selectedMonth}-15`,
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
    expect(await within(summaryCard as HTMLElement).findByText("$110.00")).toBeInTheDocument();
    expect(within(summaryCard as HTMLElement).getByText("$25.00")).toBeInTheDocument();
    expect(within(summaryCard as HTMLElement).getByText("$85.00")).toBeInTheDocument();
    expect(within(summaryCard as HTMLElement).getByText(selectedMonthLabel)).toBeInTheDocument();

    const analyticsCard = screen.getByRole("heading", { name: "Analytics" }).closest(".card");
    expect(analyticsCard).not.toBeNull();
    expect(analyticsCard).toHaveTextContent(
      "Money in: $110.00 · Money out: $25.00 · Net: $85.00",
    );

    const cumulativeNetChart = within(analyticsCard as HTMLElement).getByRole("img", {
      name: "Cumulative net chart",
    });
    const points = cumulativeNetChart.querySelector("polyline")?.getAttribute("points");
    expect(points).toBeTruthy();
    const yCoordinates = points!.split(" ").map((point) => Number(point.split(",")[1]));
    const dayBeforeRefundY = yCoordinates[13];
    const refundDayY = yCoordinates[14];
    expect(refundDayY).toBeLessThan(dayBeforeRefundY);
    expect(
      within(analyticsCard as HTMLElement).queryByRole("img", {
        name: "Spending by category chart",
      }),
    ).not.toBeInTheDocument();
  });

  it("places the minus sign before the currency symbol for negative amounts", async () => {
    const now = new Date();
    const selectedMonth = [
      now.getFullYear(),
      String(now.getMonth() + 1).padStart(2, "0"),
    ].join("-");
    apiMocks.getTransactions.mockResolvedValue([
      {
        id: "coffee-outflow",
        amount: "-25.00",
        kind: "Expense",
        category: "Food",
        date: `${selectedMonth}-10`,
        description: "Coffee purchase",
      },
    ]);

    render(<App />);

    const transactionRow = (await screen.findByText("Coffee purchase")).closest("tr");
    expect(transactionRow).not.toBeNull();
    expect(within(transactionRow as HTMLElement).getByText("-$25.00")).toBeInTheDocument();
  });

  it("treats cent-precise cancellations as zero in totals and charts", async () => {
    const now = new Date();
    const selectedMonth = [
      now.getFullYear(),
      String(now.getMonth() + 1).padStart(2, "0"),
    ].join("-");
    apiMocks.getTransactions.mockResolvedValue([
      {
        id: "first-cent-outflow",
        amount: "-0.10",
        kind: "Expense",
        category: "Food",
        date: `${selectedMonth}-10`,
        description: "First cent outflow",
      },
      {
        id: "second-cent-outflow",
        amount: "-0.20",
        kind: "Expense",
        category: "Food",
        date: `${selectedMonth}-10`,
        description: "Second cent outflow",
      },
      {
        id: "exact-cent-refund",
        amount: "0.30",
        kind: "Expense",
        category: "Food",
        date: `${selectedMonth}-10`,
        description: "Exact cent refund",
      },
    ]);

    render(<App />);

    await screen.findByText("Exact cent refund");

    const summaryCard = screen.getByRole("heading", { name: "Summary" }).closest(".card");
    expect(summaryCard).not.toBeNull();
    const netTile = within(summaryCard as HTMLElement).getByText("Net").closest(".metricTile");
    expect(netTile).not.toBeNull();
    expect(within(netTile as HTMLElement).getByText("$0.00")).toBeInTheDocument();
    expect(within(netTile as HTMLElement).queryByText("-$0.00")).not.toBeInTheDocument();

    const analyticsCard = screen.getByRole("heading", { name: "Analytics" }).closest(".card");
    expect(analyticsCard).not.toBeNull();
    expect(analyticsCard).toHaveTextContent("Net: $0.00");
    expect(
      within(analyticsCard as HTMLElement).queryByRole("img", {
        name: "Spending by category chart",
      }),
    ).not.toBeInTheDocument();

    const cumulativeNetChart = within(analyticsCard as HTMLElement).getByRole("img", {
      name: "Cumulative net chart",
    });
    const points = cumulativeNetChart.querySelector("polyline")?.getAttribute("points");
    expect(points).toBeTruthy();
    const yCoordinates = points!.split(" ").map((point) => Number(point.split(",")[1]));
    expect(new Set(yCoordinates).size).toBe(1);
  });

  it("omits an Expense category when its refunds exceed its outflows", async () => {
    const now = new Date();
    const selectedMonth = [
      now.getFullYear(),
      String(now.getMonth() + 1).padStart(2, "0"),
    ].join("-");
    apiMocks.getTransactions.mockResolvedValue([
      {
        id: "grocery-outflow",
        amount: "-10.00",
        kind: "Expense",
        category: "Groceries",
        date: `${selectedMonth}-05`,
        description: null,
      },
      {
        id: "grocery-refund",
        amount: "25.00",
        kind: "Expense",
        category: "Groceries",
        date: `${selectedMonth}-06`,
        description: null,
      },
    ]);

    render(<App />);

    const analyticsCard = screen.getByRole("heading", { name: "Analytics" }).closest(".card");
    expect(analyticsCard).not.toBeNull();
    expect(
      await within(analyticsCard as HTMLElement).findByText("No net spending for this month."),
    ).toBeInTheDocument();
    expect(
      within(analyticsCard as HTMLElement).queryByRole("img", { name: "Spending by category chart" }),
    ).not.toBeInTheDocument();
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
