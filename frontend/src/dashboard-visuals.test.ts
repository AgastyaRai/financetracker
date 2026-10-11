/// <reference types="node" />

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const appSource = readFileSync("src/App.tsx", "utf8");
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
});
