import { useId } from "react";
import { money, axisMoney } from "../format";
import { buildDayTicks, buildLineChartScale } from "../chart-scale";
import { useChartWidth } from "../hooks/useChartWidth";

/* ------------ Simple SVG Charts (no libraries) ------------ */

export function LineChart({ values, height = 160 }: { values: number[]; height?: number }) {
  const { width, chartRef } = useChartWidth(values.length > 0);
  const descriptionId = useId();
  const gradientId = `${descriptionId}-area`;
  const w = width;
  const h = height;

  if (values.length === 0) return <div className="muted">No data.</div>;

  const dataMin = Math.min(...values);
  const dataMax = Math.max(...values);
  const scale = buildLineChartScale(values);
  const chartMin = scale.min;
  const chartMax = scale.max;
  const padTop = 18;
  const padBottom = 32;
  const padLeft = 66;
  const padRight = 24;

  const innerW = w - padLeft - padRight;
  const innerH = h - padTop - padBottom;

  const xStep = values.length === 1 ? 0 : innerW / (values.length - 1);

  const toX = (i: number) => padLeft + i * xStep;
  const toY = (v: number) => {
    const t = (v - chartMin) / (chartMax - chartMin);
    return padTop + (1 - t) * innerH;
  };

  const points = values.map((v, i) => `${toX(i)},${toY(v)}`).join(" ");
  const lastIndex = values.length - 1;
  const lastValue = values[lastIndex];
  const lastX = toX(lastIndex);
  const lastY = toY(lastValue);
  const lineColor = lastValue >= 0 ? "#4ade80" : "#ff5c7a";
  const areaBaseY = dataMin >= 0
    ? padTop + innerH
    : dataMax <= 0
      ? padTop
      : toY(0);
  const areaPoints = `${padLeft},${areaBaseY} ${points} ${lastX},${areaBaseY}`;
  const yTicks = [...scale.ticks].reverse();
  const dayTicks = buildDayTicks(values.length).filter((_day, index, ticks) =>
    w >= 480 || index === 0 || index === Math.floor(ticks.length / 2) || index === ticks.length - 1,
  );

  return (
    <>
      <div ref={chartRef} className="chartViewport">
        <svg
          role="img"
          aria-label="Cumulative net chart"
          aria-describedby={descriptionId}
          viewBox={`0 0 ${w} ${h}`}
          width={w}
          height={h}
          style={{ display: "block" }}
        >
          <title>Cumulative net for the selected month</title>
          <desc id={descriptionId}>
            {`Day 1: ${money(values[0])}. Day ${values.length}: ${money(lastValue)}. Lowest: ${money(dataMin)}. Highest: ${money(dataMax)}.`}
          </desc>
          <defs>
            <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor={lineColor} stopOpacity="0.20" />
              <stop offset="100%" stopColor={lineColor} stopOpacity="0.01" />
            </linearGradient>
          </defs>
          <line
            className="lineChartAxis"
            x1={padLeft}
            y1={padTop}
            x2={padLeft}
            y2={padTop + innerH}
          />
          <line
            className="lineChartAxis"
            x1={padLeft}
            y1={padTop + innerH}
            x2={padLeft + innerW}
            y2={padTop + innerH}
          />
          {yTicks.map((tick) => (
            <g key={tick}>
              <line
                className="lineChartGrid"
                x1={padLeft}
                y1={toY(tick)}
                x2={padLeft + innerW}
                y2={toY(tick)}
              />
              <line
                className="lineChartTick"
                x1={padLeft - 5}
                y1={toY(tick)}
                x2={padLeft}
                y2={toY(tick)}
              />
              <text
                className="lineChartScale"
                x={padLeft - 10}
                y={toY(tick) + 4}
                textAnchor="end"
              >
                {axisMoney(tick)}
              </text>
            </g>
          ))}
          {dataMin < 0 && dataMax > 0 ? (
            <line
              x1={padLeft}
              y1={toY(0)}
              x2={padLeft + innerW}
              y2={toY(0)}
              stroke="rgba(255,255,255,0.30)"
            />
          ) : null}
          <polygon points={areaPoints} fill={`url(#${gradientId})`} />
          <polyline
            fill="none"
            stroke={lineColor}
            strokeWidth="3"
            strokeLinecap="round"
            strokeLinejoin="round"
            points={points}
          />
          <circle className="lineChartEndpoint" cx={lastX} cy={lastY} r="4" fill={lineColor} />
          <text
            className="lineChartEndpointValue"
            x={lastX - 8}
            y={lastY < padTop + 22 ? lastY + 22 : lastY - 10}
            textAnchor="end"
            fill={lineColor}
            fontSize="12"
            fontWeight="800"
          >
            {money(lastValue)}
          </text>
          {dayTicks.map((index) => (
            <g key={index}>
              <line
                className="lineChartTick"
                x1={toX(index)}
                y1={padTop + innerH}
                x2={toX(index)}
                y2={padTop + innerH + 5}
              />
              <text
                className="lineChartDayLabel"
                x={toX(index)}
                y={padTop + innerH + 24}
                textAnchor={index === 0 ? "start" : index === lastIndex ? "end" : "middle"}
              >
                Day {index + 1}
              </text>
            </g>
          ))}
        </svg>
      </div>
      <details className="chartDetails">
        <summary>Daily balances</summary>
        <table aria-label="Daily cumulative net balances">
          <thead><tr><th>Day</th><th>Cumulative net</th></tr></thead>
          <tbody>
            {values.map((value, index) => (
              <tr key={index}><td>Day {index + 1}</td><td>{money(value)}</td></tr>
            ))}
          </tbody>
        </table>
      </details>
    </>
  );
}

export type BarDatum = { label: string; value: number };

export function BarChart({
  data,
  height = 220,
}: {
  data: BarDatum[];
  height?: number;
}) {
  const { width, chartRef } = useChartWidth(data.length > 0);
  const descriptionId = useId();
  const minimumBand = Math.max(76, ...data.map((d) => money(d.value).length * 7));
  const w = Math.max(width, data.length * minimumBand + 30);
  const h = height;

  if (data.length === 0) return <div className="muted">No data.</div>;

  const max = Math.max(...data.map((d) => d.value), 1);

  const n = data.length;
  const rotateLabels = n > 4;
  const padTop = 22;
  const padBottom = rotateLabels ? 62 : 46; // space for rotated labels
  const padLeft = 18;
  const padRight = 12;

  const innerW = w - padLeft - padRight;
  const innerH = h - padTop - padBottom;

  const band = innerW / n;
  const barW = Math.min(120, Math.max(24, band * 0.56));

  const yFor = (v: number) => padTop + innerH * (1 - v / max);
  const hFor = (v: number) => innerH * (v / max);

  return (
    <>
      <div ref={chartRef} className="chartViewport"
        tabIndex={w > width ? 0 : undefined}
        role={w > width ? "region" : undefined}
        aria-label={w > width ? "Scrollable spending chart" : undefined}
      >
        <svg
          role="img"
          aria-label="Spending by category chart"
          aria-describedby={descriptionId}
          viewBox={`0 0 ${w} ${h}`}
          width={w}
          height={h}
          style={{ display: "block" }}
        >
          <title>Spending by category for the selected month</title>
          <desc id={descriptionId}>{data.map((d) => `${d.label}: ${money(d.value)}.`).join(" ")}</desc>

          {/* gridlines */}
          {[0.25, 0.5, 0.75, 1].map((t) => {
            const y = padTop + innerH * (1 - t);
            return (
              <line
                key={t}
                x1={padLeft}
                y1={y}
                x2={padLeft + innerW}
                y2={y}
                stroke="rgba(255,255,255,0.06)"
              />
            );
          })}

          {/* baseline */}
          <line
            x1={padLeft}
            y1={padTop + innerH}
            x2={padLeft + innerW}
            y2={padTop + innerH}
            stroke="rgba(255,255,255,0.12)"
          />

          {data.map((d, i) => {
            const xCenter = padLeft + band * i + band / 2;
            const x = xCenter - barW / 2;
            const y = yFor(d.value);
            const bh = hFor(d.value);

            // label (shorten if long)
            const maxLabelLength = Math.max(6, Math.min(14, Math.floor(band / 7)));
            const label = d.label.length > maxLabelLength ? d.label.slice(0, maxLabelLength - 2) + "…" : d.label;

            return (
              <g key={d.label}>
                <title>{`${d.label}: ${money(d.value)}`}</title>
                {/* bar */}
                <rect
                  className="categoryBar"
                  x={x}
                  y={y}
                  width={barW}
                  height={bh}
                  rx={8}
                  fill="var(--accent)"
                  opacity={0.94}
                />
                {/* value on top */}
                <text
                  className="categoryBarValue"
                  x={xCenter}
                  y={y - 6}
                  textAnchor="middle"
                  fontSize="12"
                  fill="var(--accent2)"
                  style={{ fontWeight: 700 }}
                >
                  {money(d.value)}
                </text>
                {/* rotated label */}
                <text
                  className="categoryBarLabel"
                  x={xCenter}
                  y={padTop + innerH + (rotateLabels ? 34 : 28)}
                  textAnchor="middle"
                  fontSize="12"
                  fill="rgba(244,244,245,0.90)"
                  transform={rotateLabels ? `rotate(-25 ${xCenter} ${padTop + innerH + 34})` : undefined}
                >
                  {label}
                </text>
              </g>
            );
          })}
        </svg>
      </div>
      {w > width ? <p className="muted chartScrollHint">Scroll to see all categories.</p> : null}
      <details className="chartDetails">
        <summary>Category totals</summary>
        <table aria-label="Spending category totals">
          <thead><tr><th>Category</th><th>Spent</th></tr></thead>
          <tbody>
            {data.map((d) => <tr key={d.label}><td>{d.label}</td><td>{money(d.value)}</td></tr>)}
          </tbody>
        </table>
      </details>
    </>
  );
}
