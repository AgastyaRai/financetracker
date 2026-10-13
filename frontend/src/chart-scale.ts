export type LineChartScale = {
  min: number;
  max: number;
  step: number;
  ticks: number[];
};

function niceNumber(value: number, round: boolean) {
  const exponent = Math.floor(Math.log10(value));
  const fraction = value / 10 ** exponent;
  let niceFraction: number;

  if (round) {
    if (fraction < 1.5) niceFraction = 1;
    else if (fraction < 3) niceFraction = 2;
    else if (fraction < 7) niceFraction = 5;
    else niceFraction = 10;
  } else if (fraction <= 1) niceFraction = 1;
  else if (fraction <= 2) niceFraction = 2;
  else if (fraction <= 5) niceFraction = 5;
  else niceFraction = 10;

  return niceFraction * 10 ** exponent;
}

function normalized(value: number) {
  return Number(value.toPrecision(12));
}

export function buildLineChartScale(values: number[]): LineChartScale {
  if (values.length === 0) {
    return { min: 0, max: 1, step: 0.25, ticks: [0, 0.25, 0.5, 0.75, 1] };
  }

  const dataMin = Math.min(...values);
  const dataMax = Math.max(...values);

  if (dataMin >= 0 && dataMax > 0) {
    const step = niceNumber(dataMax / 4, true);
    const min = 0;
    let max = normalized(Math.ceil(dataMax / step) * step);
    if (max <= dataMax) max = normalized(max + step);
    const tickCount = Math.round((max - min) / step);
    const ticks = Array.from({ length: tickCount + 1 }, (_, index) =>
      normalized(min + index * step),
    );

    return { min, max, step, ticks };
  }

  if (dataMin === 0 && dataMax === 0) {
    return { min: 0, max: 1, step: 0.25, ticks: [0, 0.25, 0.5, 0.75, 1] };
  }

  if (dataMax <= 0) {
    const step = niceNumber(Math.abs(dataMin) / 4, true);
    const max = 0;
    let min = normalized(Math.floor(dataMin / step) * step);
    if (min >= dataMin) min = normalized(min - step);
    const tickCount = Math.round((max - min) / step);
    const ticks = Array.from({ length: tickCount + 1 }, (_, index) =>
      normalized(min + index * step),
    );

    return { min, max, step, ticks };
  }

  const range = niceNumber(dataMax - dataMin, false);
  const step = niceNumber(range / 4, true);
  const min = normalized(Math.floor(dataMin / step) * step);
  const max = normalized(Math.ceil(dataMax / step) * step);
  const tickCount = Math.round((max - min) / step);
  const ticks = Array.from({ length: tickCount + 1 }, (_, index) =>
    normalized(min + index * step),
  );

  return { min, max, step, ticks };
}

export function buildDayTicks(dayCount: number) {
  if (dayCount <= 0) return [];

  const lastIndex = dayCount - 1;
  return Array.from(new Set([0, 0.25, 0.5, 0.75, 1].map((position) =>
    Math.floor(lastIndex * position),
  )));
}
