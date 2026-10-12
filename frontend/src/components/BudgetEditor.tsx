import type { Budget } from "../types";
import { money } from "../format";
import StatusMessageView, { type StatusMessage } from "./StatusMessageView";

type Props = {
  selectedMonth: string;
  changeSelectedMonth: (value: string) => void;
  budgetCategory: string;
  setBudgetCategory: (value: string) => void;
  budgetAmount: string;
  setBudgetAmount: (value: string) => void;
  submittingBudget: boolean;
  budgetStatus: StatusMessage | null;
  handleSaveBudget: () => Promise<void>;
  loadingBudgets: boolean;
  budgets: Budget[];
};

export default function BudgetEditor({
  selectedMonth, changeSelectedMonth, budgetCategory, setBudgetCategory, budgetAmount, setBudgetAmount, submittingBudget, budgetStatus, handleSaveBudget, loadingBudgets, budgets
}: Props) {
  return (
    <div className="card dashboardSection">
      <h2>Budgets</h2>

      <form
        className="formGrid budgetForm"
        onSubmit={(e) => { e.preventDefault(); void handleSaveBudget(); }}
      >
        <label htmlFor="budget-month">Month</label>
        <input
          id="budget-month"
          type="month"
          value={selectedMonth}
          onChange={(e) => changeSelectedMonth(e.target.value)}
        />

        <label htmlFor="budget-category">Category</label>
        <input
          id="budget-category"
          value={budgetCategory}
          onChange={(e) => setBudgetCategory(e.target.value)}
        />

        <label htmlFor="budget-amount">Amount</label>
        <input
          id="budget-amount"
          type="number"
          min="0.01"
          step="0.01"
          inputMode="decimal"
          value={budgetAmount}
          onChange={(e) => setBudgetAmount(e.target.value)}
        />

        <div className="fullRow">
          <button type="submit" disabled={submittingBudget}>
            {submittingBudget ? "Saving…" : "Save budget"}
          </button>
          <StatusMessageView message={budgetStatus} />
        </div>
      </form>

      <div style={{ marginTop: 18 }}>
        {loadingBudgets ? (
          <p className="muted">Loading budgets…</p>
        ) : budgets.length === 0 ? (
          <p className="muted">No budgets for this month yet.</p>
        ) : (
          <table>
            <thead>
              <tr>
                <th>Category</th>
                <th style={{ textAlign: "right" }}>Budget</th>
              </tr>
            </thead>
            <tbody>
              {budgets.map((b, idx) => (
                <tr key={idx}>
                  <td>{b.category}</td>
                  <td style={{ textAlign: "right" }}>{money(Number(b.amount))}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
