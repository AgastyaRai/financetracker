// frontend/src/analytics.ts
import type { Transaction } from "./types";

/**
 * Chart #1: total expenses by category
 * Returns: [{ category: "food", total: 123.45 }, ...]
 */
export function buildExpenseByCategory(transactions: Transaction[]) {
  const totals: Record<string, number> = {};

  for (const t of transactions) {
    if (t.kind !== "Expense") continue;

    const amt = Number(t.amount); // amount is a string in your API
    if (Number.isNaN(amt)) continue;

    const cat = (t.category ?? "Uncategorized").trim() || "Uncategorized";
    totals[cat] = (totals[cat] ?? 0) + amt;
  }

  return Object.entries(totals)
    .map(([category, total]) => ({ category, total: Number(total.toFixed(2)) }))
    .sort((a, b) => b.total - a.total);
}

/**
 * Chart #2: daily totals (income, expense, net) over time
 * Returns: [{ date: "2026-01-12", income: 300, expense: 12.34, net: 287.66 }, ...]
 */
export function buildDailyTotals(transactions: Transaction[]) {
  const byDate: Record<string, { income: number; expense: number }> = {};

  for (const t of transactions) {
    const amt = Number(t.amount);
    if (Number.isNaN(amt)) continue;

    const date = t.date; // already "YYYY-MM-DD"
    if (!byDate[date]) byDate[date] = { income: 0, expense: 0 };

    if (t.kind === "Income") byDate[date].income += amt;
    else byDate[date].expense += amt;
  }

  return Object.entries(byDate)
    .map(([date, v]) => {
      const income = Number(v.income.toFixed(2));
      const expense = Number(v.expense.toFixed(2));
      return { date, income, expense, net: Number((income - expense).toFixed(2)) };
    })
    .sort((a, b) => a.date.localeCompare(b.date));
}
