export type BudgetTone = "healthy" | "attention" | "over";

export function monthProgressPercent(monthInput: string, now = new Date()) {
  const year = Number(monthInput.slice(0, 4));
  const monthIndex = Number(monthInput.slice(5, 7)) - 1;
  const selectedMonthIndex = year * 12 + monthIndex;
  const currentMonthIndex = now.getFullYear() * 12 + now.getMonth();

  // Closed months have had their full budget window; future months have not started.
  if (selectedMonthIndex < currentMonthIndex) return 100;
  if (selectedMonthIndex > currentMonthIndex) return 0;

  const daysInMonth = new Date(year, monthIndex + 1, 0).getDate();
  return (now.getDate() / daysInMonth) * 100;
}

export function budgetPaceStatus(usedPercent: number, elapsedPercent: number): {
  tone: BudgetTone;
  label: "On track" | "Watch" | "Over budget";
} {
  if (usedPercent > 100) return { tone: "over", label: "Over budget" };
  if (usedPercent > elapsedPercent) return { tone: "attention", label: "Watch" };
  return { tone: "healthy", label: "On track" };
}
