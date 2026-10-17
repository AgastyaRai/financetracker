import { describe, expect, it } from "vitest";
import { buildDayTicks, buildLineChartScale } from "./chart-scale";

describe("line chart scale", () => {
  it("starts positive values at zero and leaves rounded headroom above the peak", () => {
    expect(buildLineChartScale([4200, 3888.5])).toEqual({
      min: 0,
      max: 5000,
      step: 1000,
      ticks: [0, 1000, 2000, 3000, 4000, 5000],
    });
  });

  it("keeps headroom when the peak already lands on a rounded interval", () => {
    expect(buildLineChartScale([4000, 2500])).toMatchObject({
      min: 0,
      max: 5000,
      step: 1000,
    });
  });

  it("keeps zero on a scale that crosses from negative to positive", () => {
    const scale = buildLineChartScale([-80, 120]);

    expect(scale.min).toBeLessThanOrEqual(-80);
    expect(scale.max).toBeGreaterThanOrEqual(120);
    expect(scale.ticks).toContain(0);
  });

  it("uses five evenly distributed day positions across the month", () => {
    expect(buildDayTicks(31)).toEqual([0, 7, 15, 22, 30]);
    expect(buildDayTicks(28)).toEqual([0, 6, 13, 20, 27]);
  });
});
