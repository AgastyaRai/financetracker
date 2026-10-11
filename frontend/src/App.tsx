import { useEffect, useMemo, useState, useCallback, useRef } from "react";
import "./App.css";
import type { Transaction, TransactionInput, TransactionKind, Budget, BudgetProgress, SemanticSearchResult } from "./types";
import {
  addTransaction,
  getTransactions,
  loginUser,
  registerUser,
  upsertBudget,
  getBudgets,
  getBudgetProgress,
  setUnauthorizedCallback,
  semanticSearchTransactions,
} from "./api";
import { budgetPaceStatus, monthProgressPercent } from "./budget-status";

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

type AuthMode = "login" | "register";
type StatusMessage = {
  text: string;
  type: "error" | "success";
};

function localDateInputValue(date: Date) {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

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

function monthInputLabel(monthInput: string) {
  const year = Number(monthInput.slice(0, 4));
  const month = Number(monthInput.slice(5, 7));
  return new Intl.DateTimeFormat(undefined, {
    month: "long",
    year: "numeric",
  }).format(new Date(year, month - 1, 1));
}

function money(n: number) {
  if (!Number.isFinite(n)) return "$0.00";
  const sign = n < 0 ? "-" : "";
  return `${sign}$${Math.abs(n).toFixed(2)}`;
}

/* ------------ Simple SVG Charts (no libraries) ------------ */

function LineChart({ values, height = 160 }: { values: number[]; height?: number }) {
  const w = 900;
  const h = height;

  if (values.length === 0) return <div className="muted">No data.</div>;

  let dataMin = Math.min(...values);
  let dataMax = Math.max(...values);
  if (dataMin === dataMax) {
    dataMin -= 1;
    dataMax += 1;
  }

  const dataRange = dataMax - dataMin;
  const chartMin = dataMin - dataRange * 0.12;
  const chartMax = dataMax + dataRange * 0.12;
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
  const yTicks = [dataMax, (dataMax + dataMin) / 2, dataMin];
  const dayTicks = Array.from(new Set([0, Math.floor(lastIndex / 2), lastIndex]));

  return (
    <svg
      role="img"
      aria-label="Cumulative net chart"
      viewBox={`0 0 ${w} ${h}`}
      width="100%"
      height={h}
      style={{ display: "block" }}
    >
      <title>Cumulative net for the selected month</title>
      <defs>
        <linearGradient id="lineAreaGrad" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor={lineColor} stopOpacity="0.20" />
          <stop offset="100%" stopColor={lineColor} stopOpacity="0.01" />
        </linearGradient>
      </defs>
      {yTicks.map((tick) => (
        <g key={tick}>
          <line
            x1={padLeft}
            y1={toY(tick)}
            x2={padLeft + innerW}
            y2={toY(tick)}
            stroke="rgba(255,255,255,0.08)"
          />
          <text
            className="lineChartScale"
            x={padLeft - 10}
            y={toY(tick) + 4}
            textAnchor="end"
          >
            {money(tick)}
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
      <polygon points={areaPoints} fill="url(#lineAreaGrad)" />
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
        <text
          className="lineChartDayLabel"
          key={index}
          x={toX(index)}
          y={padTop + innerH + 24}
          textAnchor={index === 0 ? "start" : index === lastIndex ? "end" : "middle"}
        >
          Day {index + 1}
        </text>
      ))}
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
    <svg
      role="img"
      aria-label="Spending by category chart"
      viewBox={`0 0 ${w} ${h}`}
      width="100%"
      height={h}
      style={{ display: "block" }}
    >
      <title>Spending by category for the selected month</title>

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
  );
}

/* ------------------------------ App ------------------------------ */

// Helper function to decode JWT and extract expiry
function parseJwtExpiry(token: string): number | null {
  try {
    const base64Url = token.split('.')[1];
    const base64 = base64Url.replace(/-/g, '+').replace(/_/g, '/');
    const jsonPayload = decodeURIComponent(
      atob(base64)
        .split('')
        .map((c) => '%' + ('00' + c.charCodeAt(0).toString(16)).slice(-2))
        .join('')
    );
    const payload = JSON.parse(jsonPayload);
    return payload.exp ? payload.exp * 1000 : null; // Convert to milliseconds
  } catch (e) {
    console.error('Failed to parse JWT:', e);
    return null;
  }
}

function hasValidStoredSession() {
  const token = localStorage.getItem("access_token");
  localStorage.removeItem("user_id");
  if (!token) return false;

  const expiryTime = parseJwtExpiry(token);
  if (expiryTime && expiryTime > Date.now()) return true;

  localStorage.removeItem("access_token");
  return false;
}

export default function App() {
  const [mode, setMode] = useState<AuthMode>("login");
  const [isAuthenticated, setIsAuthenticated] = useState<boolean>(hasValidStoredSession);
  const logoutTimerRef = useRef<number | null>(null);

  // Auth form state
  const [username, setUsername] = useState("");
  const [email, setEmail] = useState("");
  const [identifier, setIdentifier] = useState("");
  const [password, setPassword] = useState("");

  // Shared month for analytics + budgets
  const [selectedMonth, setSelectedMonth] = useState<string>(() => localDateInputValue(new Date()).slice(0, 7)); // "YYYY-MM"

  // Transactions
  const [transactions, setTransactions] = useState<Transaction[]>([]);
  const [loadingTx, setLoadingTx] = useState(false);

  // Add transaction form state
  const [amount, setAmount] = useState("12.34");
  const [kind, setKind] = useState<TransactionKind>("Expense");
  const [category, setCategory] = useState<string>("");
  const [date, setDate] = useState<string>(() => localDateInputValue(new Date()));
  const [description, setDescription] = useState("");

  // Budgets
  const [budgetCategory, setBudgetCategory] = useState("");
  const [budgetAmount, setBudgetAmount] = useState("300.00");
  const [budgets, setBudgets] = useState<Budget[]>([]);
  const [progress, setProgress] = useState<BudgetProgress[]>([]);
  const [loadingBudgets, setLoadingBudgets] = useState(false);

  // Semantic search
  const [semanticQuery, setSemanticQuery] = useState("");
  const [semanticLimit, setSemanticLimit] = useState("10");
  const [includeSummary, setIncludeSummary] = useState(false);
  const [semanticResults, setSemanticResults] = useState<SemanticSearchResult | null>(null);
  const [searchingSemantic, setSearchingSemantic] = useState(false);

  const [authStatus, setAuthStatus] = useState<StatusMessage | null>(null);
  const [transactionStatus, setTransactionStatus] = useState<StatusMessage | null>(null);
  const [budgetStatus, setBudgetStatus] = useState<StatusMessage | null>(null);
  const [transactionsStatus, setTransactionsStatus] = useState<StatusMessage | null>(null);
  const [submittingAuth, setSubmittingAuth] = useState(false);
  const [submittingTransaction, setSubmittingTransaction] = useState(false);
  const [submittingBudget, setSubmittingBudget] = useState(false);

  // Logout function - clear all auth state
  const logout = useCallback((message?: string) => {
    // Clear any existing logout timer
    if (logoutTimerRef.current) {
      clearTimeout(logoutTimerRef.current);
      logoutTimerRef.current = null;
    }
    
    localStorage.removeItem("access_token");
    localStorage.removeItem("user_id");
    setIsAuthenticated(false);
    setTransactions([]);
    setBudgets([]);
    setProgress([]);
    setPassword("");
    setTransactionStatus(null);
    setBudgetStatus(null);
    setTransactionsStatus(null);
    setSubmittingAuth(false);
    setSubmittingTransaction(false);
    setSubmittingBudget(false);
    setSearchingSemantic(false);
    setAuthStatus(message ? { text: message, type: "error" } : null);
  }, []);

  // Set up auto-logout timer based on JWT expiry
  const setupAutoLogout = useCallback((token: string) => {
    const expiryTime = parseJwtExpiry(token);
    if (!expiryTime) {
      logout("Session expired. Please log in again.");
      return false;
    }

    const now = Date.now();
    const timeUntilExpiry = expiryTime - now;

    // If already expired, logout immediately
    if (timeUntilExpiry <= 0) {
      logout("Session expired. Please log in again.");
      return false;
    }

    // Clear any existing timer
    if (logoutTimerRef.current) {
      clearTimeout(logoutTimerRef.current);
    }

    // Set up auto-logout slightly before expiry (30 seconds early)
    const logoutBuffer = 30000; // 30 seconds
    const timeUntilLogout = Math.max(0, timeUntilExpiry - logoutBuffer);

    logoutTimerRef.current = window.setTimeout(() => {
      logout("Session expired. Please log in again.");
    }, timeUntilLogout);
    return true;
  }, [logout]);

  // Set up 401 handler on mount
  useEffect(() => {
    setUnauthorizedCallback(() => logout("Session expired. Please log in again."));
  }, [logout]);

  // Set up auto-logout timer on mount if already authenticated
  useEffect(() => {
    const token = localStorage.getItem("access_token");
    if (token) {
      setupAutoLogout(token);
    }
  }, [setupAutoLogout]); // Run once on mount

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
    refreshTransactions().catch((e) => setTransactionsStatus({ text: errorMessage(e), type: "error" }));
  }, [isAuthenticated]);

  useEffect(() => {
    if (!isAuthenticated) return;
    refreshBudgets(monthStart).catch((e) => setBudgetStatus({ text: errorMessage(e), type: "error" }));
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
      const magnitude = Math.abs(v);
      if (t.kind === "Income") income += magnitude;
      else expense += magnitude;
    }
    return { income, expense, net: income - expense };
  }, [monthTx]);

  const cumulativeNetByDay = useMemo(() => {
    const days = daysInMonthFromMonthInput(selectedMonth);
    const daily = new Array<number>(days).fill(0);

    for (const t of monthTx) {
      const v = Number(t.amount);
      if (!Number.isFinite(v)) continue;
      const magnitude = Math.abs(v);

      const day = Number(t.date.slice(8, 10));
      const idx = day - 1;
      if (idx < 0 || idx >= days) continue;

      daily[idx] += t.kind === "Income" ? magnitude : -magnitude;
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
      m.set(cat, (m.get(cat) ?? 0) + Math.abs(v));
    }

    const items = Array.from(m.entries())
      .map(([label, value]) => ({ label, value }))
      .sort((a, b) => b.value - a.value);

    // top 8 is usually readable; you can bump this to 10 if you want
    return items.slice(0, 8);
  }, [monthTx]);

  // Selected-month summary
  const summary = useMemo(() => {
    let income = 0;
    let expense = 0;
    for (const t of monthTx) {
      const v = Number(t.amount);
      if (!Number.isFinite(v)) continue;
      const magnitude = Math.abs(v);
      if (t.kind === "Income") income += magnitude;
      else expense += magnitude;
    }
    return { income, expense, net: income - expense };
  }, [monthTx]);

  const selectedMonthLabel = useMemo(() => monthInputLabel(selectedMonth), [selectedMonth]);
  const budgetMonthProgress = monthProgressPercent(selectedMonth);

  // Handlers
  async function handleRegister() {
    if (submittingAuth) return;
    setAuthStatus(null);
    setSubmittingAuth(true);
    try {
      await registerUser({ username, email, password });
      setAuthStatus({ text: "Registered. Now log in.", type: "success" });
      setMode("login");
      setIdentifier(username);
    } catch (e: unknown) {
      setAuthStatus({ text: errorMessage(e), type: "error" });
    } finally {
      setSubmittingAuth(false);
    }
  }

  async function handleLogin() {
    if (submittingAuth) return;
    setAuthStatus(null);
    setSubmittingAuth(true);
    try {
      const res = await loginUser({ identifier, password });
      localStorage.setItem("access_token", res.access_token);
      
      // Set up auto-logout timer based on JWT expiry
      if (!setupAutoLogout(res.access_token)) return;
      
      setIsAuthenticated(true);
      setAuthStatus(null);
    } catch (e: unknown) {
      setAuthStatus({ text: errorMessage(e), type: "error" });
    } finally {
      setSubmittingAuth(false);
    }
  }

  async function handleAddTransaction() {
    if (!isAuthenticated) return;
    if (submittingTransaction) return;
    setTransactionStatus(null);

    if (!amount || Number(amount) <= 0) return setTransactionStatus({ text: "Amount must be > 0", type: "error" });
    if (!date) return setTransactionStatus({ text: "Date is required", type: "error" });

    const tx: TransactionInput = {
      amount,
      kind,
      category: category.trim() ? category.trim() : null,
      date,
      description: description.trim() ? description.trim() : null,
    };

    setSubmittingTransaction(true);
    try {
      await addTransaction(tx);
      setDescription("");
      setTransactionStatus({ text: "Transaction added successfully.", type: "success" });
      await refreshTransactions();
      await refreshBudgets(monthStart);
    } catch (e: unknown) {
      setTransactionStatus({ text: errorMessage(e), type: "error" });
    } finally {
      setSubmittingTransaction(false);
    }
  }

  async function handleSaveBudget() {
    if (!isAuthenticated) return;
    if (submittingBudget) return;
    setBudgetStatus(null);

    const cat = budgetCategory.trim();
    if (!cat) return setBudgetStatus({ text: "Budget category is required", type: "error" });
    if (!budgetAmount || Number(budgetAmount) <= 0) return setBudgetStatus({ text: "Budget amount must be > 0", type: "error" });

    const b: Omit<Budget, "user_id" | "id" | "created_at"> = {
      month: monthStart,
      category: cat,
      amount: budgetAmount,
    };

    setSubmittingBudget(true);
    try {
      await upsertBudget(b);
      setBudgetStatus({ text: "Budget saved.", type: "success" });
      setBudgetCategory("");
      await refreshBudgets(monthStart);
    } catch (e: unknown) {
      setBudgetStatus({ text: errorMessage(e), type: "error" });
    } finally {
      setSubmittingBudget(false);
    }
  }



  async function handleSemanticSearch() {
    if (!isAuthenticated) return;

    const query = semanticQuery.trim();

    if (!query) {
      setSemanticResults(null);
      return;
    }

    setSearchingSemantic(true);
    setTransactionsStatus(null);

    try {
      const parsedLimit = semanticLimit.trim() === "" ? undefined : Number(semanticLimit);

      const results = await semanticSearchTransactions({
        query,
        limit: parsedLimit,
        summary: includeSummary,
      });
      setSemanticResults(results);
    } catch (e: unknown) {
      setTransactionsStatus({ text: errorMessage(e), type: "error" });
    } finally {
      setSearchingSemantic(false);
    }
  }

  function clearSemanticSearch() {
    setSemanticQuery("");
    setSemanticResults(null);
    setTransactionsStatus(null);
  }

  /* ------------------------------ UI ------------------------------ */

  if (!isAuthenticated) {
    return (
      <div className="container">
        <h1>FinanceTracker</h1>

        <div className="tabs">
          <button type="button" className={mode === "login" ? "active" : ""} onClick={() => setMode("login")}>
            Login
          </button>
          <button type="button" className={mode === "register" ? "active" : ""} onClick={() => setMode("register")}>
            Register
          </button>
        </div>

        {mode === "register" ? (
          <div className="card">
            <form className="formGrid" onSubmit={(e) => { e.preventDefault(); void handleRegister(); }}>
              <label htmlFor="register-username">Username</label>
              <input
                id="register-username"
                autoComplete="username"
                required
                value={username}
                onChange={(e) => setUsername(e.target.value)}
              />

              <label htmlFor="register-email">Email</label>
              <input
                id="register-email"
                type="email"
                autoComplete="email"
                required
                value={email}
                onChange={(e) => setEmail(e.target.value)}
              />

              <label htmlFor="register-password">Password</label>
              <input
                id="register-password"
                type="password"
                autoComplete="new-password"
                required
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />

              <div className="fullRow">
                <button type="submit" disabled={submittingAuth}>Create account</button>
              </div>
            </form>
          </div>
        ) : (
          <div className="card">
            <form className="formGrid" onSubmit={(e) => { e.preventDefault(); void handleLogin(); }}>
              <label htmlFor="login-identifier">Username or Email</label>
              <input
                id="login-identifier"
                autoComplete="username"
                required
                value={identifier}
                onChange={(e) => setIdentifier(e.target.value)}
              />

              <label htmlFor="login-password">Password</label>
              <input
                id="login-password"
                type="password"
                autoComplete="current-password"
                required
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />

              <div className="fullRow">
                <button type="submit" disabled={submittingAuth}>Login</button>
              </div>
            </form>
          </div>
        )}

        {authStatus ? <p className={`status ${authStatus.type}`}>{authStatus.text}</p> : null}
      </div>
    );
  }

  const totalBudget = progress.reduce((s, p) => s + Number(p.budget_amount || 0), 0);
  const totalSpent = progress.reduce((s, p) => s + Number(p.spent || 0), 0);

  const shownSemanticTransactions = semanticResults?.transactions ?? null;

  return (
    <div className="container">
      <header className="header">
        <div className="headerTitle">
          <span className="eyebrow">FinanceTracker</span>
          <h1>Dashboard</h1>
        </div>
        <div className="headerRight">
          <button className="buttonSecondary" onClick={() => logout()}>Logout</button>
        </div>
      </header>

      {/* top grid */}
      <div className="grid">
        <div className="card transactionCard">
          <h2>Add transaction</h2>

          <form
            className="formGrid transactionForm"
            onSubmit={(e) => { e.preventDefault(); void handleAddTransaction(); }}
          >
            <label htmlFor="transaction-amount">Amount</label>
            <input
              id="transaction-amount"
              type="number"
              min="0.01"
              step="0.01"
              inputMode="decimal"
              value={amount}
              onChange={(e) => setAmount(e.target.value)}
            />

            <label htmlFor="transaction-kind">Kind</label>
            <select
              id="transaction-kind"
              value={kind}
              onChange={(e) => setKind(e.target.value as TransactionKind)}
            >
              <option value="Expense">Expense</option>
              <option value="Income">Income</option>
            </select>

            <label htmlFor="transaction-date">Date</label>
            <input
              id="transaction-date"
              type="date"
              value={date}
              onChange={(e) => setDate(e.target.value)}
            />

            <label htmlFor="transaction-category">Category (optional)</label>
            <input
              id="transaction-category"
              value={category}
              onChange={(e) => setCategory(e.target.value)}
            />

            <label htmlFor="transaction-description">Description (optional)</label>
            <input
              id="transaction-description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />

            <div className="fullRow formActions">
              <button type="submit" disabled={submittingTransaction}>Add</button>
              {transactionStatus ? (
                <p className={`status ${transactionStatus.type}`}>{transactionStatus.text}</p>
              ) : null}
            </div>
          </form>
        </div>

        <div className="card summaryCard">
          <div className="sectionHeader summaryHeader">
            <h2>Summary</h2>
            <span className="muted">{selectedMonthLabel}</span>
          </div>

          <div className="summaryMetrics">
            <div className="metricTile">
              <span className="metricLabel">Income</span>
              <strong className="metricValue income">{money(summary.income)}</strong>
            </div>
            <div className="metricTile">
              <span className="metricLabel">Expense</span>
              <strong className="metricValue expense">{money(summary.expense)}</strong>
            </div>
            <div className="metricTile">
              <span className="metricLabel">Net</span>
              <strong className="metricValue">{money(summary.net)}</strong>
            </div>
          </div>
        </div>
      </div>

      {/* 1) GENERAL CHARTS FIRST */}
      <div className="card dashboardSection">
        <div className="sectionHeader">
          <h2 style={{ margin: 0 }}>Analytics</h2>
          <div className="sectionHeaderControls">
            <label htmlFor="analytics-month" className="muted" style={{ fontWeight: 700 }}>Month</label>
            <input
              id="analytics-month"
              type="month"
              value={selectedMonth}
              onChange={(e) => setSelectedMonth(e.target.value)}
              style={{ maxWidth: 200 }}
            />
          </div>
        </div>

        {monthTx.length === 0 ? (
          <div className="emptyState">
            <strong>No activity for this month yet.</strong>
            <span>Add a transaction or choose a different month to see analytics.</span>
          </div>
        ) : (
          <>
            {/* net line chart */}
            <div className="chartSection">
              <div className="sectionHeader baseline chartHeading">
                <h3>Cumulative net</h3>
                <div className="analyticsStats">
                  <span className="analyticsStat income">
                    <span>Income</span>
                    <strong>{money(monthIncomeExpense.income)}</strong>
                  </span>
                  <span className="analyticsStat expense">
                    <span>Expense</span>
                    <strong>{money(monthIncomeExpense.expense)}</strong>
                  </span>
                  <span className="analyticsStat net">
                    <span>Net</span>
                    <strong>{money(monthIncomeExpense.net)}</strong>
                  </span>
                </div>
              </div>

              <div className="chartFrame">
                <LineChart values={cumulativeNetByDay} height={190} />
              </div>
            </div>

            {/* REAL bar chart for spending by category */}
            <div className="chartSection">
              <h3 className="chartTitle">Spending by category</h3>

              {spendingCategoryChart.length === 0 ? (
                <div className="emptyState compact">
                  <strong>No expenses for this month.</strong>
                  <span>Income activity is still reflected in cumulative net.</span>
                </div>
              ) : (
                <div className="chartFrame">
                  <BarChart data={spendingCategoryChart} height={210} />
                </div>
              )}

              {spendingCategoryChart.length > 0 ? (
                <p className="muted chartCaption">
                  Showing top categories (by total spend) for readability.
                </p>
              ) : null}
            </div>
          </>
        )}
      </div>

      {/* 2) BIG BUDGET PROGRESS SECOND */}
      <div className="card dashboardSection">
        <div className="sectionHeader baseline">
          <h2 style={{ margin: 0 }}>Budget progress</h2>
          <div className="budgetTotals">
            <span>Total spent <strong>{money(totalSpent)}</strong></span>
            <span>Total budget <strong>{money(totalBudget)}</strong></span>
          </div>
        </div>

        {loadingBudgets ? (
          <p className="muted" style={{ marginTop: 12 }}>Loading…</p>
        ) : progress.length === 0 ? (
          <p className="muted" style={{ marginTop: 12 }}>
            No progress yet — create budgets for this month, then add expense transactions with matching categories.
          </p>
        ) : (
          <div className="budgetProgressList">
            {progress.map((p) => {
              const budget = Number(p.budget_amount);
              const spent = Number(p.spent);
              const remaining = Number(p.remaining);
              const pct = budget > 0 ? (spent / budget) * 100 : 0;
              const over = pct > 100;
              const normalizedPct = Math.min(100, Math.max(0, pct));
              const { tone: budgetTone, label: budgetStatusLabel } = budgetPaceStatus(pct, budgetMonthProgress);

              return (
                <article key={p.category} className={`budgetProgressItem ${budgetTone}`}>
                  <div className="budgetProgressHeader">
                    <div className="budgetCategory">
                      <strong>{p.category}</strong>
                      <span className={`budgetStatusBadge ${budgetTone}`}>{budgetStatusLabel}</span>
                    </div>
                    <div className="budgetProgressAmount">
                      <strong>{money(spent)} <span>/</span> {money(budget)}</strong>
                      <span>{pct.toFixed(0)}% used</span>
                    </div>
                  </div>

                  <div
                    className="budgetTrack"
                    role="progressbar"
                    aria-label={`${p.category} budget used`}
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={Math.round(normalizedPct)}
                    aria-valuetext={`${pct.toFixed(0)}% used`}
                  >
                    <div
                      className={`budgetFill ${budgetTone}`}
                      style={{
                        width: `${normalizedPct}%`,
                        minWidth: spent > 0 ? "8px" : 0,
                      }}
                    />
                  </div>

                  <div className="budgetProgressFooter">
                    <span>{over ? "Over by" : "Remaining"}</span>
                    <strong>{money(Math.abs(remaining))}</strong>
                  </div>
                </article>
              );
            })}
          </div>
        )}
      </div>

      {/* 3) BUDGET EDITOR THIRD */}
      <div className="card dashboardSection">
        <h2>Budgets</h2>

        <form
          className="formGrid budgetForm"
          onSubmit={(e) => { e.preventDefault(); void handleSaveBudget(); }}
        >
          <label htmlFor="budget-month">Month</label>
          <input
            id="budget-month"
            type="month"
            value={selectedMonth}
            onChange={(e) => setSelectedMonth(e.target.value)}
          />

          <label htmlFor="budget-category">Category</label>
          <input
            id="budget-category"
            value={budgetCategory}
            onChange={(e) => setBudgetCategory(e.target.value)}
          />

          <label htmlFor="budget-amount">Amount</label>
          <input
            id="budget-amount"
            type="number"
            min="0.01"
            step="0.01"
            inputMode="decimal"
            value={budgetAmount}
            onChange={(e) => setBudgetAmount(e.target.value)}
          />

          <div className="fullRow">
            <button type="submit" disabled={submittingBudget}>Save budget</button>
            {budgetStatus ? <p className={`status ${budgetStatus.type}`}>{budgetStatus.text}</p> : null}
          </div>
        </form>

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

        <form
          style={{ display: "flex", gap: 10, marginBottom: 14, flexWrap: "wrap" }}
          onSubmit={(e) => { e.preventDefault(); void handleSemanticSearch(); }}
        >
          <input
            aria-label="Semantic search query"
            value={semanticQuery}
            onChange={(e) => setSemanticQuery(e.target.value)}
            maxLength={500}
            placeholder="Search transactions semantically (e.g. uber, groceries, ride home)"
            style={{ flex: "1 1 260px", minWidth: 0 }}
          />

          <input
            aria-label="Maximum search results"
            type="number"
            min={1}
            max={50}
            value={semanticLimit}
            onChange={(e) => setSemanticLimit(e.target.value)}
            style={{ width: 90 }}
          />

          <label style={{ display: "flex", alignItems: "center", gap: 6 }}>
            <input
              type="checkbox"
              checked={includeSummary}
              onChange={(e) => setIncludeSummary(e.target.checked)}
            />
            Summarize with AI
          </label>

          <button type="submit" disabled={searchingSemantic}>
            {searchingSemantic ? "Searching..." : "Search"}
          </button>

          <button type="button" onClick={clearSemanticSearch}>
            Clear
          </button>
        </form>

        {transactionsStatus ? (
          <p className={`status ${transactionsStatus.type}`}>{transactionsStatus.text}</p>
        ) : null}

        {semanticResults?.summary && (
          <div className="card" style={{ marginBottom: 14 }}>
            <h3>AI Summary</h3>
            <p>{semanticResults.summary}</p>
          </div>
        )}

        {shownSemanticTransactions ? (
          shownSemanticTransactions.length === 0 ? (
            <p className="muted">No matching transactions found.</p>
          ) : (
            <div className="tableScroll">
              <table>
                <thead>
                  <tr>
                    <th>Date</th>
                    <th>Kind</th>
                    <th>Category</th>
                    <th>Description</th>
                    <th>Similarity</th>
                    <th style={{ textAlign: "right" }}>Amount</th>
                  </tr>
                </thead>
                <tbody>
                  {shownSemanticTransactions.map((match) => (
                    <tr key={match.transaction.id}>
                      <td>{match.transaction.date}</td>
                      <td>{match.transaction.kind}</td>
                      <td>{match.transaction.category ?? "-"}</td>
                      <td>{match.transaction.description ?? "-"}</td>
                      <td>{(match.similarity_score * 100).toFixed(1)}%</td>
                      <td style={{ textAlign: "right" }}>
                        {money(Number(match.transaction.amount))}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )
        ) : loadingTx ? (
          <p className="muted">Loading…</p>
        ) : transactions.length === 0 ? (
          <p className="muted">No transactions yet.</p>
        ) : (
          <div className="tableScroll">
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
                {transactions.map((t) => (
                  <tr key={t.id}>
                    <td>{t.date}</td>
                    <td>{t.kind}</td>
                    <td>{t.category ?? "-"}</td>
                    <td>{t.description ?? "-"}</td>
                    <td style={{ textAlign: "right" }}>{money(Number(t.amount))}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </div>
  );
}
