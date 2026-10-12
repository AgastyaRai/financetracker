/// <reference types="node" />

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const appSource = [
  "src/App.tsx", "src/components/Charts.tsx", "src/components/AnalyticsSection.tsx",
  "src/components/BudgetProgressSection.tsx",
].map((path) => readFileSync(path, "utf8")).join("\n");
const appCss = readFileSync("src/App.css", "utf8");
const indexCss = readFileSync("src/index.css", "utf8");

describe("dashboard visual hierarchy", () => {
  it("caps category bars so sparse data stays proportionate", () => {
    expect(appSource).toContain("Math.min(120, Math.max(24, band * 0.56))");
  });

  it("uses structured analytics stats and budget progress cards", () => {
    expect(appSource).toContain('className="analyticsStats"');
    expect(appSource).toContain("budgetProgressItem");
    expect(appSource).toContain('role="progressbar"');
    expect(appCss).toContain(".analyticsStats{");
    expect(appCss).toContain(".budgetProgressItem{");
    expect(appCss).toContain(".budgetTrack{");
  });

  it("replaces the retired gradient palette with semantic colors", () => {
    const themeSource = `${appSource}\n${appCss}\n${indexCss}`;
    expect(themeSource).not.toMatch(/124,92,255|34,211,238|#7c5cff|#22d3ee/i);
    expect(indexCss).toContain("--warning: #f5b942;");
    expect(appCss).toContain(".budgetFill.healthy{ background: var(--success); }");
    expect(appCss).toContain(".budgetFill.attention{ background: var(--warning); }");
    expect(appCss).toContain(".budgetFill.over{ background: var(--danger); }");
  });

  it("adds readable scale cues and semantic budget states", () => {
    expect(appSource).toContain("buildLineChartScale(values)");
    expect(appSource).toContain("buildDayTicks(values.length)");
    expect(appSource).toContain('className="lineChartAxis"');
    expect(appSource).toContain('className="lineChartTick"');
    expect(appSource).toContain('className="lineChartScale"');
    expect(appSource).toContain('className="lineChartDayLabel"');
    expect(appSource).toContain('className="lineChartEndpoint"');
    expect(appSource).toContain("monthProgressPercent(selectedMonth)");
    expect(appSource).toContain("budgetPaceStatus(pct, budgetMonthProgress)");
  });

  it("uses a deep indigo brand accent for identity and category spending", () => {
    expect(indexCss).toContain("--accent: #33418f;");
    expect(indexCss).toContain("--accent2: #7380c9;");
    expect(appSource).toContain('fill="var(--accent)"');
    expect(appSource).toContain('className="categoryBarValue"');
    expect(appSource).toContain('className="categoryBarLabel"');
    expect(appCss).toContain("border-left: 3px solid var(--accent);");
  });

  it("uses continuous bold separators for table columns without a spreadsheet grid", () => {
    expect(appCss).toContain("thead th:not(:last-child),");
    expect(appCss).toContain("tbody td:not(:last-child){");
    expect(appCss).toContain("border-right: 2px solid var(--border-strong);");
    expect(appCss).not.toContain("border-bottom: 1px solid rgba(255,255,255,0.06);");
    expect(appCss).not.toContain("tbody td:not(:last-child)::after");
  });

  it("gives form controls consistent corners and stronger outlines", () => {
    const buttonCss = appCss.split("\nbutton{")[1]?.split("}")[0] ?? "";
    expect(buttonCss).toContain("border-radius: 6px;");
    expect(buttonCss).toContain("border: 2px solid var(--border-strong);");
  });

  it("separates major card frames from internal control borders", () => {
    const cardCss = appCss.match(/\.card\{([^}]+)\}/)?.[1] ?? "";
    expect(indexCss).toContain("--card: #0c0e15;");
    expect(indexCss).toContain("--section-border: #111216;");
    expect(indexCss).toContain("--section-accent: #586ab6;");
    expect(cardCss).toContain("linear-gradient(90deg, var(--section-accent), var(--border-strong) 12%, #1c2030 45%, var(--section-border) 78%) border-box");
    expect(cardCss).toContain("border-left-width: 4px;");
    expect(cardCss).not.toContain("inset");
  });
});
