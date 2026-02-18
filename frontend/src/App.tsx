import { useEffect, useMemo, useState } from "react";
import "./App.css";
import type { Transaction, TransactionKind, Budget, BudgetProgress } from "./types";
import {
  addTransaction,
  getTransactions,
  loginUser,
  registerUser,
  upsertBudget,
  getBudgets,
  getBudgetProgress,
} from "./api";

type AuthMode = "login" | "register";

function monthInputToMonthStart(monthInput: string) {
  // "2026-01" -> "2026-01-01"
  return `${monthInput}-01`;
}

function nextMonthStart(monthStart: string) {
  // monthStart: "YYYY-MM-01"
  const y = Number(monthStart.slice(0, 4));
  const m = Number(monthStart.slice(5, 7));
  const ny = m === 12 ? y + 1 : y;
  const nm = m === 12 ? 1 : m + 1;
  const mm = String(nm).padStart(2, "0");
  return `${ny}-${mm}-01`;
}

function daysInMonthFromMonthInput(monthInput: string) {
  const y = Number(monthInput.slice(0, 4));
  const m = Number(monthInput.slice(5, 7));
  return new Date(y, m, 0).getDate();
}

function money(n: number) {
  if (!Number.isFinite(n)) return "$0.00";
  return `$${n.toFixed(2)}`;
}

/* ------------ Simple SVG Charts (no libraries) ------------ */

function LineChart({ values, height = 160 }: { values: number[]; height?: number }) {
  const w = 900;
  const h = height;

  if (values.length === 0) return <div className="muted">No data.</div>;

  let min = Math.min(...values);
  let max = Math.max(...values);
  if (min === max) {
    min -= 1;
    max += 1;
  }

  const padTop = 12;
  const padBottom = 18;
  const padLeft = 10;
  const padRight = 10;

  const innerW = w - padLeft - padRight;
  const innerH = h - padTop - padBottom;

  const xStep = values.length === 1 ? 0 : innerW / (values.length - 1);

  const toX = (i: number) => padLeft + i * xStep;
  const toY = (v: number) => {
    const t = (v - min) / (max - min);
    return padTop + (1 - t) * innerH;
  };

  const points = values.map((v, i) => `${toX(i)},${toY(v)}`).join(" ");

  return (
    <svg viewBox={`0 0 ${w} ${h}`} width="100%" height={h} style={{ display: "block" }}>
      <line
        x1={padLeft}
        y1={padTop + innerH}
        x2={padLeft + innerW}
        y2={padTop + innerH}
        stroke="rgba(255,255,255,0.10)"
      />
      <polyline fill="none" stroke="rgba(34,211,238,0.95)" strokeWidth="3" points={points} />
      <polyline
        fill="none"
        stroke="rgba(124,92,255,0.55)"
        strokeWidth="6"
        opacity="0.35"
        points={points}
      />
    </svg>
  );
}

type BarDatum = { label: string; value: number };

function BarChart({
  data,
  height = 220,
}: {
  data: BarDatum[];
  height?: number;
}) {
  const w = 900;
  const h = height;

  if (data.length === 0) return <div className="muted">No data.</div>;

  const max = Math.max(...data.map((d) => d.value), 1);

  const padTop = 14;
  const padBottom = 62; // space for rotated labels
  const padLeft = 18;
  const padRight = 12;

  const innerW = w - padLeft - padRight;
  const innerH = h - padTop - padBottom;

  const n = data.length;
  const band = innerW / n;
  const barW = Math.max(10, band * 0.62);

  const yFor = (v: number) => padTop + innerH * (1 - v / max);
  const hFor = (v: number) => innerH * (v / max);

  return (
    <svg viewBox={`0 0 ${w} ${h}`} width="100%" height={h} style={{ display: "block" }}>
      <defs>
        <linearGradient id="barGrad" x1="0" y1="0" x2="1" y2="0">
          <stop offset="0%" stopColor="rgba(124,92,255,0.95)" />
          <stop offset="100%" stopColor="rgba(34,211,238,0.85)" />
        </linearGradient>
      </defs>

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
        const label = d.label.length > 14 ? d.label.slice(0, 12) + "…" : d.label;

        return (
          <g key={d.label}>
            {/* bar */}
            <rect
              x={x}
              y={y}
              width={barW}
              height={bh}
              rx={10}
              fill="url(#barGrad)"
              opacity={0.95}
            />
            {/* value on top */}
            <text
              x={xCenter}
              y={y - 6}
              textAnchor="middle"
              fontSize="12"
              fill="rgba(233,234,242,0.85)"
              style={{ fontWeight: 700 }}
            >
              {money(d.value)}
            </text>
            {/* rotated label */}
            <text
              x={xCenter}
              y={padTop + innerH + 34}
              textAnchor="middle"
              fontSize="12"
              fill="rgba(233,234,242,0.75)"
              transform={`rotate(-25 ${xCenter} ${padTop + innerH + 34})`}
            >
              {label}
            </text>
          </g>
        );
      })}
    </svg>
  );
}

/* ------------------------------ App ------------------------------ */

export default function App() {
  const [mode, setMode] = useState<AuthMode>("login");
  const [isAuthenticated, setIsAuthenticated] = useState<boolean>(() => !!localStorage.getItem("access_token"));

  // Auth form state
  const [username, setUsername] = useState("");
  const [email, setEmail] = useState("");
  const [identifier, setIdentifier] = useState("");
  const [password, setPassword] = useState("");

  // Shared month for analytics + budgets
  const [selectedMonth, setSelectedMonth] = useState<string>(() => new Date().toISOString().slice(0, 7)); // "YYYY-MM"

  // Transactions
  const [transactions, setTransactions] = useState<Transaction[]>([]);
  const [loadingTx, setLoadingTx] = useState(false);

  // Add transaction form state
  const [amount, setAmount] = useState("12.34");
  const [kind, setKind] = useState<TransactionKind>("Expense");
  const [category, setCategory] = useState<string>("");
  const [date, setDate] = useState<string>(() => new Date().toISOString().slice(0, 10));
  const [description, setDescription] = useState("");

  // Budgets
  const [budgetCategory, setBudgetCategory] = useState("");
  const [budgetAmount, setBudgetAmount] = useState("300.00");
  const [budgets, setBudgets] = useState<Budget[]>([]);
  const [progress, setProgress] = useState<BudgetProgress[]>([]);
  const [loadingBudgets, setLoadingBudgets] = useState(false);

  const [status, setStatus] = useState<string>("");

  const monthStart = useMemo(() => monthInputToMonthStart(selectedMonth), [selectedMonth]);
  const monthEnd = useMemo(() => nextMonthStart(monthStart), [monthStart]);

  async function refreshTransactions() {
    setLoadingTx(true);
    try {
      const txs = await getTransactions();
      txs.sort((a, b) => b.date.localeCompare(a.date));
      setTransactions(txs);
    } finally {
      setLoadingTx(false);
    }
  }

  async function refreshBudgets(monthStartStr: string) {
    setLoadingBudgets(true);
    try {
      const bs = await getBudgets(monthStartStr);
      bs.sort((a, b) => a.category.localeCompare(b.category));
      setBudgets(bs);

      const p = await getBudgetProgress(monthStartStr);
      p.sort((a, b) => a.category.localeCompare(b.category));
      setProgress(p);
    } finally {
      setLoadingBudgets(false);
    }
  }

  useEffect(() => {
    if (!isAuthenticated) return;
    refreshTransactions().catch((e) => setStatus(e.message));
  }, [isAuthenticated]);

  useEffect(() => {
    if (!isAuthenticated) return;
    refreshBudgets(monthStart).catch((e) => setStatus(e.message));
  }, [isAuthenticated, monthStart]);

  // Month-filtered transactions for analytics
  const monthTx = useMemo(() => {
    return transactions.filter((t) => t.date >= monthStart && t.date < monthEnd);
  }, [transactions, monthStart, monthEnd]);

  const monthIncomeExpense = useMemo(() => {
    let income = 0;
    let expense = 0;
    for (const t of monthTx) {
      const v = Number(t.amount);
      if (!Number.isFinite(v)) continue;
      if (t.kind === "Income") income += v;
      else expense += v;
    }
    return { income, expense, net: income - expense };
  }, [monthTx]);

  const cumulativeNetByDay = useMemo(() => {
    const days = daysInMonthFromMonthInput(selectedMonth);
    const daily = new Array<number>(days).fill(0);

    for (const t of monthTx) {
      const v = Number(t.amount);
      if (!Number.isFinite(v)) continue;

      const day = Number(t.date.slice(8, 10));
      const idx = day - 1;
      if (idx < 0 || idx >= days) continue;

      daily[idx] += t.kind === "Income" ? v : -v;
    }

    const cum: number[] = [];
    let s = 0;
    for (const d of daily) {
      s += d;
      cum.push(s);
    }
    return cum;
  }, [monthTx, selectedMonth]);

  // Spending by category -> real bar chart (compare by height)
  const spendingCategoryChart = useMemo(() => {
    const m = new Map<string, number>();

    for (const t of monthTx) {
      if (t.kind !== "Expense") continue;
      const v = Number(t.amount);
      if (!Number.isFinite(v)) continue;
      const cat = (t.category ?? "Uncategorized").trim() || "Uncategorized";
      m.set(cat, (m.get(cat) ?? 0) + v);
    }

    const items = Array.from(m.entries())
      .map(([label, value]) => ({ label, value }))
      .sort((a, b) => b.value - a.value);

    // top 8 is usually readable; you can bump this to 10 if you want
    return items.slice(0, 8);
  }, [monthTx]);

  // Overall (all-time) summary
  const summary = useMemo(() => {
    let income = 0;
    let expense = 0;
    for (const t of transactions) {
      const v = Number(t.amount);
      if (!Number.isFinite(v)) continue;
      if (t.kind === "Income") income += v;
      else expense += v;
    }
    return { income, expense, net: income - expense };
  }, [transactions]);

  // Handlers
  async function handleRegister() {
    setStatus("");
    try {
      await registerUser({ username, email, password });
      setStatus("Registered. Now log in.");
      setMode("login");
      setIdentifier(username);
    } catch (e: any) {
      setStatus(e.message);
    }
  }

  async function handleLogin() {
    setStatus("");
    try {
      const res = await loginUser({ identifier, password });
      localStorage.setItem("access_token", res.access_token);
      setIsAuthenticated(true);
      setStatus("");
    } catch (e: any) {
      setStatus(e.message);
    }
  }

  async function handleAddTransaction() {
    if (!isAuthenticated) return;
    setStatus("");

    if (!amount || Number(amount) <= 0) return setStatus("Amount must be > 0");
    if (!date) return setStatus("Date is required");

    const tx: Omit<Transaction, "user_id" | "id" | "created_at"> = {
      amount,
      kind,
      category: category.trim() ? category.trim() : null,
      date,
      description: description.trim() ? description.trim() : null,
    };

    try {
      await addTransaction(tx);
      setDescription("");
      setStatus("Transaction added successfully.");
      await refreshTransactions();
      await refreshBudgets(monthStart);
    } catch (e: any) {
      setStatus(e.message);
    }
  }

  async function handleSaveBudget() {
    if (!isAuthenticated) return;
    setStatus("");

    const cat = budgetCategory.trim();
    if (!cat) return setStatus("Budget category is required");
    if (!budgetAmount || Number(budgetAmount) <= 0) return setStatus("Budget amount must be > 0");

    const b: Omit<Budget, "user_id" | "id" | "created_at"> = {
      month: monthStart,
      category: cat,
      amount: budgetAmount,
    };

    try {
      await upsertBudget(b);
      setStatus("Budget saved.");
      setBudgetCategory("");
      await refreshBudgets(monthStart);
    } catch (e: any) {
      setStatus(e.message);
    }
  }

  function logout() {
    localStorage.removeItem("access_token");
    setIsAuthenticated(false);
    setTransactions([]);
    setBudgets([]);
    setProgress([]);
    setPassword("");
    setStatus("");
  }

  /* ------------------------------ UI ------------------------------ */

  if (!isAuthenticated) {
    return (
      <div className="container">
        <h1>FinanceTracker</h1>

        <div className="tabs">
          <button className={mode === "login" ? "active" : ""} onClick={() => setMode("login")}>
            Login
          </button>
          <button className={mode === "register" ? "active" : ""} onClick={() => setMode("register")}>
            Register
          </button>
        </div>

        {mode === "register" ? (
          <div className="card">
            <div className="formGrid">
              <label>Username</label>
              <input value={username} onChange={(e) => setUsername(e.target.value)} />

              <label>Email</label>
              <input value={email} onChange={(e) => setEmail(e.target.value)} />

              <label>Password</label>
              <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} />

              <div className="fullRow">
                <button onClick={handleRegister}>Create account</button>
              </div>
            </div>
          </div>
        ) : (
          <div className="card">
            <div className="formGrid">
              <label>Username or Email</label>
              <input value={identifier} onChange={(e) => setIdentifier(e.target.value)} />

              <label>Password</label>
              <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} />

              <div className="fullRow">
                <button onClick={handleLogin}>Login</button>
              </div>
            </div>
          </div>
        )}

        {status ? <p className="status">{status}</p> : null}
      </div>
    );
  }

  const totalBudget = progress.reduce((s, p) => s + Number(p.budget_amount || 0), 0);
  const totalSpent = progress.reduce((s, p) => s + Number(p.spent || 0), 0);

  return (
    <div className="container">
      <header className="header">
        <h1 style={{ textAlign: "left", fontSize: 48, margin: 0 }}>Dashboard</h1>
        <div className="headerRight">
          <button onClick={logout}>Logout</button>
        </div>
      </header>

      {/* top grid */}
      <div className="grid">
        <div className="card">
          <h2>Add transaction</h2>

          <div className="formGrid" style={{ width: "100%", margin: 0 }}>
            <label>Amount</label>
            <input value={amount} onChange={(e) => setAmount(e.target.value)} />

            <label>Kind</label>
            <select value={kind} onChange={(e) => setKind(e.target.value as TransactionKind)}>
              <option value="Expense">Expense</option>
              <option value="Income">Income</option>
            </select>

            <label>Date</label>
            <input type="date" value={date} onChange={(e) => setDate(e.target.value)} />

            <label>Category (optional)</label>
            <input value={category} onChange={(e) => setCategory(e.target.value)} />

            <label>Description (optional)</label>
            <input value={description} onChange={(e) => setDescription(e.target.value)} />

            <div className="fullRow">
              <button onClick={handleAddTransaction}>Add</button>
              {status ? <p className="status">{status}</p> : null}
            </div>
          </div>
        </div>

        <div className="card">
          <h2>Summary</h2>
          <div className="summaryRow">
            <span>Income</span>
            <span>{money(summary.income)}</span>
          </div>
          <div className="summaryRow">
            <span>Expense</span>
            <span>{money(summary.expense)}</span>
          </div>
          <div className="summaryRow strong">
            <span>Net</span>
            <span>{money(summary.net)}</span>
          </div>
        </div>
      </div>

      {/* 1) GENERAL CHARTS FIRST */}
      <div className="card" style={{ marginBottom: 16 }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 12 }}>
          <h2 style={{ margin: 0 }}>Analytics</h2>
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <span className="muted" style={{ fontWeight: 700 }}>Month</span>
            <input
              type="month"
              value={selectedMonth}
              onChange={(e) => setSelectedMonth(e.target.value)}
              style={{ maxWidth: 200 }}
            />
          </div>
        </div>

        {/* net line chart */}
        <div style={{ marginTop: 14 }}>
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "baseline", gap: 12 }}>
            <h3 style={{ margin: "0 0 6px" }}>Cumulative net (this month)</h3>
            <span className="muted">
              Income: {money(monthIncomeExpense.income)} · Expense: {money(monthIncomeExpense.expense)} · Net:{" "}
              <b>{money(monthIncomeExpense.net)}</b>
            </span>
          </div>

          <div
            style={{
              borderRadius: 14,
              border: "1px solid rgba(255,255,255,0.10)",
              background: "rgba(0,0,0,0.18)",
              padding: 10,
            }}
          >
            <LineChart values={cumulativeNetByDay} height={170} />
          </div>
        </div>

        {/* REAL bar chart for spending by category */}
        <div style={{ marginTop: 18 }}>
          <h3 style={{ margin: "0 0 8px" }}>Spending by category (this month)</h3>

          {spendingCategoryChart.length === 0 ? (
            <p className="muted">No expenses yet for this month.</p>
          ) : (
            <div
              style={{
                borderRadius: 14,
                border: "1px solid rgba(255,255,255,0.10)",
                background: "rgba(0,0,0,0.18)",
                padding: 10,
              }}
            >
              <BarChart data={spendingCategoryChart} height={240} />
            </div>
          )}

          <p className="muted" style={{ marginTop: 8 }}>
            Showing top categories (by total spend) for readability.
          </p>
        </div>
      </div>

      {/* 2) BIG BUDGET PROGRESS SECOND */}
      <div className="card" style={{ marginBottom: 16 }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "baseline", gap: 12 }}>
          <h2 style={{ margin: 0 }}>Budget progress</h2>
          <span className="muted">
            Total spent: <b>{money(totalSpent)}</b> · Total budget: <b>{money(totalBudget)}</b>
          </span>
        </div>

        {loadingBudgets ? (
          <p className="muted" style={{ marginTop: 12 }}>Loading…</p>
        ) : progress.length === 0 ? (
          <p className="muted" style={{ marginTop: 12 }}>
            No progress yet — create budgets for this month, then add expense transactions with matching categories.
          </p>
        ) : (
          <div style={{ marginTop: 14, display: "grid", gap: 14 }}>
            {progress.map((p) => {
              const budget = Number(p.budget_amount);
              const spent = Number(p.spent);
              const remaining = Number(p.remaining);
              const pct = budget > 0 ? (spent / budget) * 100 : 0;
              const over = pct > 100;

              return (
                <div key={p.category} style={{ display: "grid", gap: 8 }}>
                  <div style={{ display: "flex", justifyContent: "space-between", gap: 12 }}>
                    <div style={{ display: "flex", alignItems: "baseline", gap: 10 }}>
                      <span style={{ fontWeight: 900, fontSize: 16 }}>{p.category}</span>
                      {over ? (
                        <span style={{ color: "rgb(255, 99, 132)", fontWeight: 900 }}>Over budget</span>
                      ) : null}
                    </div>
                    <span className="muted" style={{ fontWeight: 700 }}>
                      {money(spent)} / {money(budget)} ({pct.toFixed(0)}%)
                    </span>
                  </div>

                  <div
                    style={{
                      height: 18,
                      borderRadius: 999,
                      border: "1px solid rgba(255,255,255,0.14)",
                      background: "rgba(255,255,255,0.06)",
                      overflow: "hidden",
                    }}
                  >
                    <div
                      style={{
                        width: `${Math.min(100, Math.max(0, pct))}%`,
                        height: "100%",
                        borderRadius: 999,
                        background: over
                          ? "linear-gradient(90deg, rgba(255,99,132,0.95), rgba(255,159,64,0.85))"
                          : "linear-gradient(90deg, rgba(124,92,255,0.90), rgba(34,211,238,0.80))",
                      }}
                    />
                  </div>

                  <span className="muted">Remaining: {money(remaining)}</span>
                </div>
              );
            })}
          </div>
        )}
      </div>

      {/* 3) BUDGET EDITOR THIRD */}
      <div className="card" style={{ marginBottom: 16 }}>
        <h2>Budgets</h2>

        <div className="formGrid" style={{ width: "100%", margin: 0 }}>
          <label>Month</label>
          <input type="month" value={selectedMonth} onChange={(e) => setSelectedMonth(e.target.value)} />

          <label>Category</label>
          <input value={budgetCategory} onChange={(e) => setBudgetCategory(e.target.value)} />

          <label>Amount</label>
          <input value={budgetAmount} onChange={(e) => setBudgetAmount(e.target.value)} />

          <div className="fullRow">
            <button onClick={handleSaveBudget}>Save budget</button>
          </div>
        </div>

        <div style={{ marginTop: 14 }}>
          {loadingBudgets ? (
            <p className="muted">Loading budgets…</p>
          ) : budgets.length === 0 ? (
            <p className="muted">No budgets for this month yet.</p>
          ) : (
            <table>
              <thead>
                <tr>
                  <th>Category</th>
                  <th style={{ textAlign: "right" }}>Budget</th>
                </tr>
              </thead>
              <tbody>
                {budgets.map((b, idx) => (
                  <tr key={idx}>
                    <td>{b.category}</td>
                    <td style={{ textAlign: "right" }}>{money(Number(b.amount))}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </div>

      {/* 4) TRANSACTIONS LAST */}
      <div className="card">
        <h2>Transactions</h2>
        {loadingTx ? (
          <p className="muted">Loading…</p>
        ) : transactions.length === 0 ? (
          <p className="muted">No transactions yet.</p>
        ) : (
          <table>
            <thead>
              <tr>
                <th>Date</th>
                <th>Kind</th>
                <th>Category</th>
                <th>Description</th>
                <th style={{ textAlign: "right" }}>Amount</th>
              </tr>
            </thead>
            <tbody>
              {transactions.map((t, idx) => (
                <tr key={idx}>
                  <td>{t.date}</td>
                  <td>{t.kind}</td>
                  <td>{t.category ?? "-"}</td>
                  <td>{t.description ?? "-"}</td>
                  <td style={{ textAlign: "right" }}>{money(Number(t.amount))}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
