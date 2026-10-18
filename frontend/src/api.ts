import type { LoginResponse, Transaction, TransactionInput, UpdateTransactionInput, Budget, BudgetProgress, SemanticSearchResult, IndexingStatus } from "./types";

import type { BankLinkToken, BankConnectionResponse, BankImportResponse } from "./types";

// If using Vite proxy, keep API_BASE = "/api"
const API_BASE = "/api";

// Logout callback - will be set by App component
let onUnauthorized: (() => void) | null = null;

export function setUnauthorizedCallback(callback: (() => void) | null) {
  onUnauthorized = callback;
}

function responseErrorMessage(text: string, status: number) {
  if (!text) return `Request failed (${status})`;

  try {
    const body = JSON.parse(text) as { error?: unknown; message?: unknown };
    if (typeof body.error === "string") return body.error;
    if (typeof body.message === "string") return body.message;
  } catch {
    // Plain-text responses are already suitable for display.
  }

  return text;
}

// keep the HTTP status so the editor can recognize a stale-save conflict.
export class ApiError extends Error {
  readonly status: number;

  constructor(message: string, status: number) {
    super(message);
    this.status = status;
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const isAuthRequest = path === "/users/login" || path === "/users/register";
  const token = isAuthRequest ? null : localStorage.getItem("access_token");
  const res = await fetch(`${API_BASE}${path}`, {
    ...init,
    headers: {
      "Content-Type": "application/json",
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...(init?.headers ?? {}),
    },
  });

  // Handle 401 Unauthorized - session expired or invalid token
  if (res.status === 401 && !isAuthRequest) {
    // An old response must not log out a session established while it was pending.
    if (token && token === localStorage.getItem("access_token")) {
      // Clear auth data
      localStorage.removeItem("access_token");
      localStorage.removeItem("user_id");

      // Trigger logout callback
      if (onUnauthorized) {
        onUnauthorized();
      }
    }
    
    throw new Error("Session expired. Please log in again.");
  }

  if (!res.ok) {
    const msg = await res.text().catch(() => "");
    throw new ApiError(responseErrorMessage(msg, res.status), res.status);
  }

  const text = await res.text();
  if (!text) return undefined as T;
  return JSON.parse(text) as T;
}

export async function registerUser(input: { username: string; email: string; password: string }) {
  await request<void>("/users/register", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function loginUser(input: { identifier: string; password: string }) {
  return await request<LoginResponse>("/users/login", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function addTransaction(tx: TransactionInput): Promise<void> {
  await request<void>("/transactions", {
    method: "POST",
    body: JSON.stringify(tx),
  });
}

export async function updateTransaction(id: string, tx: UpdateTransactionInput): Promise<void> {
  await request<void>(`/transactions/${encodeURIComponent(id)}`, {
    method: "PUT",
    body: JSON.stringify(tx),
  });
}

export async function revertTransaction(id: string) {
  return await request<Pick<Transaction, "kind" | "category" | "description">>(`/transactions/${encodeURIComponent(id)}/revert`, {
    method: "POST",
  });
}

export async function getTransactions() {
  return await request<Transaction[]>("/transactions", { method: "GET" });
}

export async function getIndexingStatus(signal?: AbortSignal) {
  return await request<IndexingStatus>("/transactions/indexing-status", { method: "GET", signal });
}


/* bank connections */

export async function getBankConnections() {
  return await request<BankConnectionResponse[]>("/bank-connections", { method: "GET" });
}

export async function createBankLinkToken() {
  return await request<BankLinkToken>("/bank-connections/link-token", { method: "POST" });
}

export async function exchangeBankToken(publicToken: string) {
  return await request<BankConnectionResponse>("/bank-connections/exchange-token", {
    method: "POST",
    body: JSON.stringify({ public_token: publicToken }),
  });
}

export async function importBankTransactions(connectionId: string) {
  return await request<BankImportResponse>(`/bank-connections/${encodeURIComponent(connectionId)}/import`, { method: "POST" });
}

/* budgets */

export async function upsertBudget(budget: Budget): Promise<void> {
  await request<void>("/budgets", {
    method: "POST",
    body: JSON.stringify(budget),
  });
}

export async function getBudgets(month?: string): Promise<Budget[]> {
  const q = month ? `?month=${encodeURIComponent(month)}` : "";
  return await request<Budget[]>(`/budgets${q}`, { method: "GET" });
}

export async function getBudgetProgress(month?: string): Promise<BudgetProgress[]> {
  const q = month ? `?month=${encodeURIComponent(month)}` : "";
  return await request<BudgetProgress[]>(`/budgets/progress${q}`, { method: "GET" });
}

export async function semanticSearchTransactions(input: {
  query: string;
  limit?: number;
  summary?: boolean;
}): Promise<SemanticSearchResult> {
  return await request<SemanticSearchResult>("/transactions/search/semantic", {
    method: "POST",
    body: JSON.stringify(input),
  });
}
