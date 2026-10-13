import { useEffect, useMemo, useState, useCallback, useRef } from "react";
import "./App.css";
import type { StatusMessage } from "./components/StatusMessageView";
import AuthScreen from "./components/AuthScreen";
import TransactionForm from "./components/TransactionForm";
import SummaryCard from "./components/SummaryCard";
import AnalyticsSection from "./components/AnalyticsSection";
import BudgetProgressSection from "./components/BudgetProgressSection";
import BudgetEditor from "./components/BudgetEditor";
import TransactionsSection from "./components/TransactionsSection";
import { useRequestLifecycle } from "./hooks/useRequestLifecycle";
import type { Transaction, TransactionInput, TransactionKind, Budget, BudgetProgress, SemanticSearchResult } from "./types";
import {
  addTransaction,
  updateTransaction,
  revertTransaction,
  getTransactions,
  loginUser,
  registerUser,
  upsertBudget,
  getBudgets,
  getBudgetProgress,
  setUnauthorizedCallback,
  semanticSearchTransactions,
} from "./api";
import { monthProgressPercent } from "./budget-status";

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

type AuthMode = "login" | "register";
type SemanticSearchInput = Parameters<typeof semanticSearchTransactions>[0];
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
  const checkSessionRef = useRef<(() => void) | null>(null);

  // Auth form state
  const [username, setUsername] = useState("");
  const [email, setEmail] = useState("");
  const [identifier, setIdentifier] = useState("");
  const [password, setPassword] = useState("");

  // Shared month for analytics + budgets
  const [selectedMonth, setSelectedMonth] = useState<string>(() => localDateInputValue(new Date()).slice(0, 7)); // "YYYY-MM"

  const selectedMonthStartRef = useRef(monthInputToMonthStart(selectedMonth));

  // Transactions
  const [transactions, setTransactions] = useState<Transaction[]>([]);
  const [loadingTx, setLoadingTx] = useState(false);
  const [editingTransaction, setEditingTransaction] = useState<Transaction | null>(null);
  const [submittingEdit, setSubmittingEdit] = useState(false);
  const [revertingTransactionId, setRevertingTransactionId] = useState<string | null>(null);
  const [editStatus, setEditStatus] = useState<StatusMessage | null>(null);

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
  const activeSemanticSearch = useRef<SemanticSearchInput | null>(null);

  const [authStatus, setAuthStatus] = useState<StatusMessage | null>(null);
  const [transactionStatus, setTransactionStatus] = useState<StatusMessage | null>(null);
  const [budgetStatus, setBudgetStatus] = useState<StatusMessage | null>(null);
  const [transactionsStatus, setTransactionsStatus] = useState<StatusMessage | null>(null);
  const [submittingAuth, setSubmittingAuth] = useState(false);
  const [submittingTransaction, setSubmittingTransaction] = useState(false);
  const [submittingBudget, setSubmittingBudget] = useState(false);

  const { requestVersions, captureSession, beginRequest } = useRequestLifecycle();

  // Logout function - clear all auth state
  const logout = useCallback((message?: string, clearStoredSession = true) => {
    requestVersions.current.session++;
    checkSessionRef.current = null;
    // Clear any existing logout timer
    if (logoutTimerRef.current) {
      clearTimeout(logoutTimerRef.current);
      logoutTimerRef.current = null;
    }
    
    if (clearStoredSession) {
      localStorage.removeItem("access_token");
      localStorage.removeItem("user_id");
    }
    setIsAuthenticated(false);
    setTransactions([]);
    setEditingTransaction(null);
    setSubmittingEdit(false);
    setRevertingTransactionId(null);
    setEditStatus(null);
    activeSemanticSearch.current = null;
    setBudgets([]);
    setProgress([]);
    setLoadingTx(false);
    setLoadingBudgets(false);
    setAmount("12.34");
    setKind("Expense");
    setCategory("");
    setDate(localDateInputValue(new Date()));
    setDescription("");
    setBudgetCategory("");
    setBudgetAmount("300.00");
    setSemanticQuery("");
    setSemanticLimit("10");
    setIncludeSummary(false);
    setSemanticResults(null);
    setPassword("");
    setTransactionStatus(null);
    setBudgetStatus(null);
    setTransactionsStatus(null);
    setSubmittingAuth(false);
    setSubmittingTransaction(false);
    setSubmittingBudget(false);
    setSearchingSemantic(false);
    setAuthStatus(message ? { text: message, type: "error" } : null);
  }, [requestVersions]);

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
    const logoutDeadline = expiryTime - logoutBuffer;

    function checkSession() {
      if (localStorage.getItem("access_token") !== token) {
        logout("Session changed in another tab. Please log in again.", false);
        return;
      }
      // Suspension and clock changes can separate elapsed timer time from the JWT deadline.
      const remaining = logoutDeadline - Date.now();
      if (remaining <= 0) {
        logout("Session expired. Please log in again.");
        return;
      }
      if (logoutTimerRef.current) clearTimeout(logoutTimerRef.current);
      logoutTimerRef.current = window.setTimeout(checkSession, Math.min(remaining, 2_147_483_647));
    }
    checkSessionRef.current = checkSession;
    logoutTimerRef.current = window.setTimeout(checkSession, Math.min(timeUntilLogout, 2_147_483_647));
    return true;
  }, [logout]);

  // Set up 401 handler on mount
  useEffect(() => {
    const versions = requestVersions.current;
    setUnauthorizedCallback(() => logout("Session expired. Please log in again."));
    return () => {
      setUnauthorizedCallback(null);
      versions.session++;
    };
  }, [logout, requestVersions]);

  // Set up auto-logout timer on mount if already authenticated
  useEffect(() => {
    const token = localStorage.getItem("access_token");
    if (token) {
      setupAutoLogout(token);
    }
    return () => {
      if (logoutTimerRef.current) clearTimeout(logoutTimerRef.current);
      logoutTimerRef.current = null;
      checkSessionRef.current = null;
    };
  }, [setupAutoLogout]); // Run once on mount

  useEffect(() => {
    function checkSessionOnResume() {
      checkSessionRef.current?.();
    }
    function checkVisibleSession() {
      if (document.visibilityState === "visible") checkSessionOnResume();
    }
    window.addEventListener("focus", checkSessionOnResume);
    window.addEventListener("pageshow", checkSessionOnResume);
    document.addEventListener("visibilitychange", checkVisibleSession);
    return () => {
      window.removeEventListener("focus", checkSessionOnResume);
      window.removeEventListener("pageshow", checkSessionOnResume);
      document.removeEventListener("visibilitychange", checkVisibleSession);
    };
  }, []);

  useEffect(() => {
    function handleSessionChange(event: StorageEvent) {
      if (event.storageArea !== localStorage) return;
      if (event.key !== "access_token" && event.key !== null) return;
      const token = localStorage.getItem("access_token");
      if (event.key === "access_token" && (event.newValue !== token || event.newValue === event.oldValue)) return;
      logout(token ? "Session changed in another tab. Please log in again." : "Logged out in another tab.", false);
    }
    window.addEventListener("storage", handleSessionChange);
    return () => window.removeEventListener("storage", handleSessionChange);
  }, [logout]);

  const monthStart = useMemo(() => monthInputToMonthStart(selectedMonth), [selectedMonth]);
  const monthEnd = useMemo(() => nextMonthStart(monthStart), [monthStart]);

  function changeSelectedMonth(value: string) {
    if (!/^\d{4,}-(0[1-9]|1[0-2])$/.test(value)) return;
    if (value === selectedMonth) return;
    selectedMonthStartRef.current = monthInputToMonthStart(value);
    setBudgets([]);
    setProgress([]);
    setBudgetStatus(null);
    setSelectedMonth(value);
  }

  const refreshTransactions = useCallback(async () => {
    const isCurrent = beginRequest("transactions");
    setLoadingTx(true);
    try {
      const txs = await getTransactions();
      if (!isCurrent()) return;
      setTransactions([...txs].sort((a, b) => b.date.localeCompare(a.date)));
    } catch (e) {
      if (isCurrent()) throw e;
    } finally {
      if (isCurrent()) setLoadingTx(false);
    }
  }, [beginRequest]);

  const refreshBudgets = useCallback(async (monthStartStr: string) => {
    if (monthStartStr !== selectedMonthStartRef.current) return;
    const isCurrentRequest = beginRequest("budgets");
    const isCurrent = () => isCurrentRequest() && monthStartStr === selectedMonthStartRef.current;
    setLoadingBudgets(true);
    try {
      const [bs, p] = await Promise.all([getBudgets(monthStartStr), getBudgetProgress(monthStartStr)]);
      if (!isCurrent()) return;
      setBudgets([...bs].sort((a, b) => a.category.localeCompare(b.category)));
      setProgress([...p].sort((a, b) => a.category.localeCompare(b.category)));
    } catch (e) {
      if (isCurrent()) throw e;
    } finally {
      if (isCurrent()) setLoadingBudgets(false);
    }
  }, [beginRequest]);

  useEffect(() => {
    if (!isAuthenticated) return;
    const isCurrentSession = captureSession();
    refreshTransactions().catch((e) => {
      if (isCurrentSession()) setTransactionsStatus({ text: errorMessage(e), type: "error" });
    });
  }, [isAuthenticated, captureSession, refreshTransactions]);

  useEffect(() => {
    if (!isAuthenticated) return;
    const isCurrentSession = captureSession();
    refreshBudgets(monthStart).catch((e) => {
      if (isCurrentSession() && monthStart === selectedMonthStartRef.current) {
        setBudgetStatus({ text: errorMessage(e), type: "error" });
      }
    });
  }, [isAuthenticated, monthStart, captureSession, refreshBudgets]);

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
  // The summary and analytics header intentionally share one selected-month total.
  const summary = monthIncomeExpense;

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
    let isCurrentSession = captureSession();
    setAuthStatus(null);
    setSubmittingAuth(true);
    try {
      const res = await loginUser({ identifier, password });
      if (!isCurrentSession()) return;
      localStorage.setItem("access_token", res.access_token);
      isCurrentSession = captureSession();
      
      // Set up auto-logout timer based on JWT expiry
      if (!setupAutoLogout(res.access_token)) return;
      
      setIsAuthenticated(true);
      setAuthStatus(null);
    } catch (e: unknown) {
      if (isCurrentSession()) setAuthStatus({ text: errorMessage(e), type: "error" });
    } finally {
      if (isCurrentSession()) setSubmittingAuth(false);
    }
  }

  async function handleAddTransaction() {
    if (!isAuthenticated) return;
    if (submittingTransaction) return;
    setTransactionStatus(null);
    setTransactionsStatus(null);

    if (!amount || Number(amount) <= 0) return setTransactionStatus({ text: "Amount must be > 0", type: "error" });
    if (!date) return setTransactionStatus({ text: "Date is required", type: "error" });

    const tx: TransactionInput = {
      amount,
      kind,
      category: category.trim() ? category.trim() : null,
      date,
      description: description.trim() ? description.trim() : null,
    };

    const isCurrentSession = captureSession();
    setSubmittingTransaction(true);
    try {
      await addTransaction(tx);
      if (!isCurrentSession()) return;
      setDescription((current) => current === description ? "" : current);
      setTransactionStatus({ text: "Transaction added successfully.", type: "success" });
    } catch (e: unknown) {
      if (isCurrentSession()) {
        setTransactionStatus({ text: errorMessage(e), type: "error" });
        setSubmittingTransaction(false);
      }
      return;
    }

    try {
      await Promise.all([refreshTransactions(), refreshBudgets(selectedMonthStartRef.current)]);
    } catch (e: unknown) {
      if (isCurrentSession()) setTransactionsStatus({
        text: `Transaction was saved, but the dashboard could not refresh: ${errorMessage(e)}`,
        type: "error",
      });
    } finally {
      if (isCurrentSession()) setSubmittingTransaction(false);
    }
  }

  function startEditingTransaction(transaction: Transaction) {
    if (submittingEdit) return;
    setEditingTransaction(transaction);
    setEditStatus(null);
    setTransactionsStatus(null);
  }

  function cancelEditingTransaction() {
    if (submittingEdit) return;
    setEditingTransaction(null);
    setEditStatus(null);
  }

  async function handleUpdateTransaction(input: TransactionInput) {
    if (editingTransaction) await handleTransactionWrite(editingTransaction, input);
  }

  async function handleRevertTransaction(transaction: Transaction) {
    if (editingTransaction || !transaction.can_revert) return;
    await handleTransactionWrite(transaction);
  }

  async function handleTransactionWrite(transaction: Transaction, input?: TransactionInput) {
    if (!isAuthenticated || submittingEdit) return;
    setEditStatus(null);
    if (input && (!input.amount.trim() || !Number.isFinite(Number(input.amount)) || Number(input.amount) <= 0)) {
      setEditStatus({ text: "Amount must be greater than zero", type: "error" });
      return;
    }
    if (input && !input.date) {
      setEditStatus({ text: "Date is required", type: "error" });
      return;
    }

    const transactionId = transaction.id;
    const isRevert = !input;
    const isCurrentSession = captureSession();
    let savedInput: TransactionInput;
    setSubmittingEdit(true);
    setRevertingTransactionId(isRevert ? transactionId : null);
    setTransactionsStatus(null);
    try {
      if (input) {
        await updateTransaction(transactionId, input);
        savedInput = input;
      } else {
        const originalFields = await revertTransaction(transactionId);
        savedInput = { amount: transaction.amount, date: transaction.date, ...originalFields };
      }
      if (!isCurrentSession()) return;
    } catch (e: unknown) {
      if (isCurrentSession()) {
        if (isRevert) setTransactionsStatus({ text: errorMessage(e), type: "error" });
        else setEditStatus({ text: errorMessage(e), type: "error" });
        setSubmittingEdit(false);
        setRevertingTransactionId(null);
      }
      return;
    }

    // A completed write invalidates reads and AI summaries that used the previous transaction fields.
    requestVersions.current.transactions++;
    requestVersions.current.search++;
    setTransactions((current) => {
      const updated = { id: transactionId, ...savedInput, ...(isRevert ? { can_revert: false } : {}) };
      const next = current.some((transaction) => transaction.id === transactionId)
        ? current.map((transaction) => transaction.id === transactionId ? { ...transaction, ...updated } : transaction)
        : [...current, updated];
      return next.sort((a, b) => b.date.localeCompare(a.date));
    });
    setSemanticResults(null);
    setSearchingSemantic(false);
    setEditingTransaction(null);
    setTransactionsStatus({ text: isRevert ? "Imported fields reverted successfully." : "Transaction updated successfully.", type: "success" });

    const refreshMonth = selectedMonthStartRef.current;
    const refreshes = [refreshTransactions(), refreshBudgets(refreshMonth)];
    const refreshKinds: Array<"transactions" | "budgets" | "search"> = ["transactions", "budgets"];
    const searchInput = activeSemanticSearch.current;
    if (searchInput) {
      refreshes.push(refreshSemanticSearch(searchInput));
      refreshKinds.push("search");
    }
    const refreshVersions = { ...requestVersions.current };
    const results = await Promise.allSettled(refreshes);
    if (!isCurrentSession()) return;
    // Clear or a newer month can supersede one refresh while the other dashboard reads are pending.
    const failure = results.find((result, index) => {
      const requestKind = refreshKinds[index];
      return result.status === "rejected"
        && requestVersions.current[requestKind] === refreshVersions[requestKind]
        && (requestKind !== "budgets" || refreshMonth === selectedMonthStartRef.current);
    });
    if (failure?.status === "rejected") {
      setTransactionsStatus({
        text: `Transaction was ${isRevert ? "reverted" : "updated"}, but the dashboard could not refresh: ${errorMessage(failure.reason)}`,
        type: "error",
      });
    }
    setSubmittingEdit(false);
    setRevertingTransactionId(null);
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

    const isCurrentSession = captureSession();
    const isCurrentMonth = () => isCurrentSession() && monthStart === selectedMonthStartRef.current;
    setSubmittingBudget(true);
    try {
      await upsertBudget(b);
      if (!isCurrentMonth()) {
        if (isCurrentSession()) setSubmittingBudget(false);
        return;
      }
      setBudgetStatus({ text: "Budget saved.", type: "success" });
      setBudgetCategory((current) => current === budgetCategory ? "" : current);
    } catch (e: unknown) {
      if (isCurrentMonth()) setBudgetStatus({ text: errorMessage(e), type: "error" });
      if (isCurrentSession()) setSubmittingBudget(false);
      return;
    }

    try {
      await refreshBudgets(monthStart);
    } catch (e: unknown) {
      if (isCurrentMonth()) setBudgetStatus({
        text: `Budget was saved, but the dashboard could not refresh: ${errorMessage(e)}`,
        type: "error",
      });
    } finally {
      if (isCurrentSession()) setSubmittingBudget(false);
    }
  }



  async function handleSemanticSearch() {
    if (!isAuthenticated || searchingSemantic || submittingEdit) return;

    const query = semanticQuery.trim();

    if (!query) {
      clearSemanticSearch();
      return;
    }

    const input = {
      query,
      limit: semanticLimit.trim() === "" ? undefined : Number(semanticLimit),
      summary: includeSummary,
    };
    activeSemanticSearch.current = input;
    setTransactionsStatus(null);
    const isCurrentSession = captureSession();
    const request = refreshSemanticSearch(input);
    const version = requestVersions.current.search;
    try {
      await request;
    } catch (e: unknown) {
      if (isCurrentSession() && version === requestVersions.current.search) {
        setTransactionsStatus({ text: errorMessage(e), type: "error" });
      }
    }
  }

  async function refreshSemanticSearch(input: SemanticSearchInput) {
    const isCurrent = beginRequest("search");
    setSearchingSemantic(true);
    setSemanticResults(null);
    try {
      const results = await semanticSearchTransactions(input);
      if (isCurrent()) setSemanticResults(results);
    } catch (e: unknown) {
      if (isCurrent()) {
        activeSemanticSearch.current = null;
        throw e;
      }
    } finally {
      if (isCurrent()) setSearchingSemantic(false);
    }
  }

  function clearSemanticSearch() {
    activeSemanticSearch.current = null;
    requestVersions.current.search++;
    setSearchingSemantic(false);
    setSemanticQuery("");
    setSemanticResults(null);
    setTransactionsStatus(null);
  }

  /* ------------------------------ UI ------------------------------ */

  if (!isAuthenticated) {
    return (
      <AuthScreen
        mode={mode} setMode={setMode}
        username={username} setUsername={setUsername}
        email={email} setEmail={setEmail}
        identifier={identifier} setIdentifier={setIdentifier}
        password={password} setPassword={setPassword}
        submittingAuth={submittingAuth} authStatus={authStatus}
        handleRegister={handleRegister} handleLogin={handleLogin}
      />
    );
  }

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
        <TransactionForm
          amount={amount} setAmount={setAmount} kind={kind} setKind={setKind}
          date={date} setDate={setDate} category={category} setCategory={setCategory}
          description={description} setDescription={setDescription}
          submittingTransaction={submittingTransaction} transactionStatus={transactionStatus}
          handleAddTransaction={handleAddTransaction}
        />

        <SummaryCard selectedMonthLabel={selectedMonthLabel} summary={summary} />
      </div>

      {/* 1) GENERAL CHARTS FIRST */}
      <AnalyticsSection
        selectedMonth={selectedMonth} changeSelectedMonth={changeSelectedMonth}
        monthTx={monthTx} monthIncomeExpense={monthIncomeExpense}
        cumulativeNetByDay={cumulativeNetByDay} spendingCategoryChart={spendingCategoryChart}
      />

      {/* 2) BIG BUDGET PROGRESS SECOND */}
      <BudgetProgressSection
        progress={progress} loadingBudgets={loadingBudgets} budgetMonthProgress={budgetMonthProgress}
      />

      {/* 3) BUDGET EDITOR THIRD */}
      <BudgetEditor
        selectedMonth={selectedMonth} changeSelectedMonth={changeSelectedMonth}
        budgetCategory={budgetCategory} setBudgetCategory={setBudgetCategory}
        budgetAmount={budgetAmount} setBudgetAmount={setBudgetAmount}
        submittingBudget={submittingBudget} budgetStatus={budgetStatus} handleSaveBudget={handleSaveBudget}
        loadingBudgets={loadingBudgets} budgets={budgets}
      />

      {/* 4) TRANSACTIONS LAST */}
      <TransactionsSection
        semanticQuery={semanticQuery} setSemanticQuery={setSemanticQuery}
        semanticLimit={semanticLimit} setSemanticLimit={setSemanticLimit}
        includeSummary={includeSummary} setIncludeSummary={setIncludeSummary}
        searchingSemantic={searchingSemantic} handleSemanticSearch={handleSemanticSearch}
        clearSemanticSearch={clearSemanticSearch} transactionsStatus={transactionsStatus}
        semanticResults={semanticResults} loadingTx={loadingTx} transactions={transactions}
        editingTransaction={editingTransaction} submittingEdit={submittingEdit} editStatus={editStatus}
        startEditingTransaction={startEditingTransaction} cancelEditingTransaction={cancelEditingTransaction}
        handleUpdateTransaction={handleUpdateTransaction}
        handleRevertTransaction={handleRevertTransaction} revertingTransactionId={revertingTransactionId}
      />
    </div>
  );
}
