import { describe, expect, it } from "vitest";
import { buildDailyTotals, buildExpenseByCategory } from "./analytics";
import type { Transaction } from "./types";

describe("analytics calculations", () => {
  it("uses Expense as the classification and signed amounts to return positive net spending by category", () => {
    const transactions: Transaction[] = [
      {
        id: "grocery-outflow",
        amount: "-80.00",
        kind: "Expense",
        category: "Groceries",
        date: "2026-03-05",
        description: null,
      },
      {
        id: "grocery-refund",
        amount: "20.00",
        kind: "Expense",
        category: "Groceries",
        date: "2026-03-06",
        description: null,
      },
      {
        id: "income-classified-outflow",
        amount: "-30.00",
        kind: "Income",
        category: "Groceries",
        date: "2026-03-07",
        description: null,
      },
      {
        id: "travel-outflow",
        amount: "-10.00",
        kind: "Expense",
        category: "Travel",
        date: "2026-03-08",
        description: null,
      },
      {
        id: "travel-refund",
        amount: "25.00",
        kind: "Expense",
        category: "Travel",
        date: "2026-03-09",
        description: null,
      },
    ];

    expect(buildExpenseByCategory(transactions)).toEqual([
      { category: "Groceries", total: 60 },
    ]);
  });

  it("uses the amount sign instead of kind to calculate daily money flow", () => {
    const transactions: Transaction[] = [
      {
        id: "expense-classified-inflow",
        amount: "100.00",
        kind: "Expense",
        category: "Food",
        date: "2026-03-10",
        description: null,
      },
      {
        id: "income-classified-outflow",
        amount: "-25.00",
        kind: "Income",
        category: "Pay",
        date: "2026-03-10",
        description: null,
      },
    ];

    expect(buildDailyTotals(transactions)).toEqual([
      { date: "2026-03-10", income: 100, expense: 25, net: 75 },
    ]);
  });
});
