import type { Transaction } from "../types";
import { money } from "../format";
import { LineChart, BarChart, type BarDatum } from "./Charts";

type Props = {
  selectedMonth: string;
  changeSelectedMonth: (value: string) => void;
  monthTx: Transaction[];
  monthIncomeExpense: { income: number; expense: number; net: number };
  cumulativeNetByDay: number[];
  spendingCategoryChart: BarDatum[];
};

export default function AnalyticsSection({
  selectedMonth, changeSelectedMonth, monthTx, monthIncomeExpense, cumulativeNetByDay, spendingCategoryChart
}: Props) {
  return (
    <div className="card dashboardSection">
      <div className="sectionHeader">
        <h2 style={{ margin: 0 }}>Analytics</h2>
        <div className="sectionHeaderControls">
          <label htmlFor="analytics-month" className="muted" style={{ fontWeight: 700 }}>Month</label>
          <input
            id="analytics-month"
            type="month"
            value={selectedMonth}
            onChange={(e) => changeSelectedMonth(e.target.value)}
            style={{ maxWidth: 200 }}
          />
        </div>
      </div>

      {monthTx.length === 0 ? (
        <div className="emptyState">
          <strong>No activity for this month yet.</strong>
          <span>Add a transaction or choose a different month to see analytics.</span>
        </div>
      ) : (
        <>
          {/* net line chart */}
          <div className="chartSection">
            <div className="sectionHeader baseline chartHeading">
              <h3>Cumulative net</h3>
              <div className="analyticsStats">
                <span className="analyticsStat income">
                  <span>Income</span>
                  <strong>{money(monthIncomeExpense.income)}</strong>
                </span>
                <span className="analyticsStat expense">
                  <span>Expense</span>
                  <strong>{money(monthIncomeExpense.expense)}</strong>
                </span>
                <span className="analyticsStat net">
                  <span>Net</span>
                  <strong>{money(monthIncomeExpense.net)}</strong>
                </span>
              </div>
            </div>

            <div className="chartFrame">
              <LineChart values={cumulativeNetByDay} height={190} />
            </div>
          </div>

          {/* REAL bar chart for spending by category */}
          <div className="chartSection">
            <h3 className="chartTitle">Spending by category</h3>

            {spendingCategoryChart.length === 0 ? (
              <div className="emptyState compact">
                <strong>No expenses for this month.</strong>
                <span>Income activity is still reflected in cumulative net.</span>
              </div>
            ) : (
              <div className="chartFrame">
                <BarChart data={spendingCategoryChart} height={210} />
              </div>
            )}

            {spendingCategoryChart.length > 0 ? (
              <p className="muted chartCaption">
                Showing top categories (by total spend) for readability.
              </p>
            ) : null}
          </div>
        </>
      )}
    </div>
  );
}
