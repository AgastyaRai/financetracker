import { money } from "../format";

type Props = {
  selectedMonthLabel: string;
  summary: { income: number; expense: number; net: number };
};

export default function SummaryCard({
  selectedMonthLabel, summary
}: Props) {
  return (
    <div className="card summaryCard">
      <div className="sectionHeader summaryHeader">
        <h2>Summary</h2>
        <span className="muted">{selectedMonthLabel}</span>
      </div>

      <div className="summaryMetrics">
        <div className="metricTile">
          <span className="metricLabel">Money in</span>
          <strong className="metricValue income">{money(summary.income)}</strong>
        </div>
        <div className="metricTile">
          <span className="metricLabel">Money out</span>
          <strong className="metricValue expense">{money(summary.expense)}</strong>
        </div>
        <div className="metricTile">
          <span className="metricLabel">Net</span>
          <strong className="metricValue">{money(summary.net)}</strong>
        </div>
      </div>
    </div>
  );
}
