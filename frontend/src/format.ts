export function money(n: number) {
  if (!Number.isFinite(n)) return "$0.00";
  const rounded = Number(n.toFixed(2));
  const sign = rounded < 0 ? "-" : "";
  return `${sign}$${Math.abs(rounded).toFixed(2)}`;
}

export function axisMoney(n: number) {
  const sign = n < 0 ? "-" : "";
  return `${sign}$${Math.abs(n).toLocaleString("en-US", { maximumFractionDigits: 2 })}`;
}
