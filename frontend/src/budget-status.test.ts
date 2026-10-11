import { describe, expect, it } from "vitest";
import { budgetPaceStatus, monthProgressPercent } from "./budget-status";

describe("budget pace status", () => {
  it("measures the current month by the local calendar day", () => {
    const now = new Date(2026, 9, 10, 12);

    expect(monthProgressPercent("2026-10", now)).toBeCloseTo((10 / 31) * 100);
  });

  it("treats past months as complete and future months as not started", () => {
    const now = new Date(2026, 9, 10, 12);

    expect(monthProgressPercent("2026-09", now)).toBe(100);
    expect(monthProgressPercent("2026-11", now)).toBe(0);
  });

  it("marks usage at or below elapsed time as on track", () => {
    expect(budgetPaceStatus(32, 32.26)).toEqual({ tone: "healthy", label: "On track" });
    expect(budgetPaceStatus(100, 100)).toEqual({ tone: "healthy", label: "On track" });
  });

  it("marks usage ahead of elapsed time for attention", () => {
    expect(budgetPaceStatus(33, 32.26)).toEqual({ tone: "attention", label: "Watch" });
    expect(budgetPaceStatus(1, 0)).toEqual({ tone: "attention", label: "Watch" });
  });

  it("keeps over-budget usage as the highest-severity state", () => {
    expect(budgetPaceStatus(101, 100)).toEqual({ tone: "over", label: "Over budget" });
  });
});
