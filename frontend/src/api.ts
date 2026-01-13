import type { LoginResponse, Transaction, Budget, BudgetProgress } from "./types";

// If using Vite proxy, keep API_BASE = "/api"
const API_BASE = "/api";

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`, {
    ...init,
    headers: {
      "Content-Type": "application/json",
      ...(init?.headers ?? {}),
    },
  });

  if (!res.ok) {
    const msg = await res.text().catch(() => "");
    throw new Error(msg || `Request failed (${res.status})`);
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

export async function addTransaction(tx: Transaction): Promise<void> {
  await request<void>("/transactions", {
    method: "POST",
    body: JSON.stringify(tx),
  });
}

export async function getTransactions(userId: string) {
  return await request<Transaction[]>(`/transactions/${userId}`, { method: "GET" });
}

/* budgets */

export async function upsertBudget(budget: Budget): Promise<void> {
  await request<void>("/budgets", {
    method: "POST",
    body: JSON.stringify(budget),
  });
}

export async function getBudgets(userId: string, month?: string): Promise<Budget[]> {
  const q = month ? `?month=${encodeURIComponent(month)}` : "";
  return await request<Budget[]>(`/budgets/${userId}${q}`, { method: "GET" });
}

export async function getBudgetProgress(userId: string, month?: string): Promise<BudgetProgress[]> {
  const q = month ? `?month=${encodeURIComponent(month)}` : "";
  return await request<BudgetProgress[]>(`/budgets/${userId}/progress${q}`, { method: "GET" });
}
