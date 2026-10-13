import { useEffect, useRef, useState } from "react";

export function useChartWidth(hasData: boolean) {
  const chartRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(900);

  useEffect(() => {
    const element = chartRef.current;
    if (!element || typeof ResizeObserver === "undefined") return;

    // Observe the viewport, so a wider scrollable SVG cannot grow its own measurement.
    const observer = new ResizeObserver(([entry]) => {
      if (entry && entry.contentRect.width > 0) setWidth(Math.floor(entry.contentRect.width));
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [hasData]);

  return { width, chartRef };
}
