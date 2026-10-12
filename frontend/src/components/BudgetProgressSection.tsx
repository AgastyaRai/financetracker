import type { BudgetProgress } from "../types";
import { budgetPaceStatus } from "../budget-status";
import { money } from "../format";

type Props = {
  progress: BudgetProgress[];
  loadingBudgets: boolean;
  budgetMonthProgress: number;
};

export default function BudgetProgressSection({
  progress, loadingBudgets, budgetMonthProgress
}: Props) {
  const totalBudget = progress.reduce((s, p) => s + Number(p.budget_amount || 0), 0);
  const totalSpent = progress.reduce((s, p) => s + Number(p.spent || 0), 0);

  return (
    <div className="card dashboardSection">
      <div className="sectionHeader baseline">
        <h2 style={{ margin: 0 }}>Budget progress</h2>
        <div className="budgetTotals">
          <span>Total spent <strong>{money(totalSpent)}</strong></span>
          <span>Total budget <strong>{money(totalBudget)}</strong></span>
        </div>
      </div>

      {loadingBudgets ? (
        <p className="muted" style={{ marginTop: 12 }}>Loading…</p>
      ) : progress.length === 0 ? (
        <p className="muted" style={{ marginTop: 12 }}>
          No progress yet — create budgets for this month, then add expense transactions with matching categories.
        </p>
      ) : (
        <div className="budgetProgressList">
          {progress.map((p) => {
            const budget = Number(p.budget_amount);
            const spent = Number(p.spent);
            const remaining = Number(p.remaining);
            const pct = budget > 0 ? (spent / budget) * 100 : 0;
            const over = pct > 100;
            const normalizedPct = Math.min(100, Math.max(0, pct));
            const { tone: budgetTone, label: budgetStatusLabel } = budgetPaceStatus(pct, budgetMonthProgress);

            return (
              <article key={p.category} className={`budgetProgressItem ${budgetTone}`}>
                <div className="budgetProgressHeader">
                  <div className="budgetCategory">
                    <strong>{p.category}</strong>
                    <span className={`budgetStatusBadge ${budgetTone}`}>{budgetStatusLabel}</span>
                  </div>
                  <div className="budgetProgressAmount">
                    <strong>{money(spent)} <span>/</span> {money(budget)}</strong>
                    <span>{pct.toFixed(0)}% used</span>
                  </div>
                </div>

                <div
                  className="budgetTrack"
                  role="progressbar"
                  aria-label={`${p.category} budget used`}
                  aria-valuemin={0}
                  aria-valuemax={100}
                  aria-valuenow={Math.round(normalizedPct)}
                  aria-valuetext={`${pct.toFixed(0)}% used`}
                >
                  <div
                    className={`budgetFill ${budgetTone}`}
                    style={{
                      width: `${normalizedPct}%`,
                      minWidth: spent > 0 ? "8px" : 0,
                    }}
                  />
                </div>

                <div className="budgetProgressFooter">
                  <span>{over ? "Over by" : "Remaining"}</span>
                  <strong>{money(Math.abs(remaining))}</strong>
                </div>
              </article>
            );
          })}
        </div>
      )}
    </div>
  );
}
