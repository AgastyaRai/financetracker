import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createMockApiServer } from "./mock-api.mjs";

let server;

async function authenticatedFetch(url, init) {
  const origin = new URL(url).origin;
  const login = await fetch(`${origin}/api/users/login`, { method: "POST" });
  const { access_token: token } = await login.json();
  return fetch(url, { ...init, headers: { Authorization: `Bearer ${token}`, ...init?.headers } });
}

afterEach(async () => {
  if (!server) return;
  await new Promise((resolve, reject) => {
    server.close((error) => error ? reject(error) : resolve());
  });
  server = undefined;
});

describe("preview session expiry", () => {
  let now = Date.parse("2026-10-12T12:00:00Z");
  let base;

  beforeEach(async () => {
    now = Date.parse("2026-10-12T12:00:00Z");
    server = createMockApiServer({}, { now: () => now });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    base = `http://127.0.0.1:${server.address().port}/api`;
  });

  async function login() {
    const response = await fetch(`${base}/users/login`, { method: "POST" });
    expect(response.status).toBe(200);
    return (await response.json()).access_token;
  }

  it("issues a fresh 24-hour session for each login even after a day of uptime", async () => {
    const first = await login();
    expect(await login()).not.toBe(first);
    now += 86_400_000;
    const second = await login();
    const expiry = JSON.parse(Buffer.from(second.split(".")[1], "base64url").toString()).exp;
    expect(expiry).toBe(Math.floor(now / 1000) + 86400);
    expect(second).not.toBe(first);
  });

  it("rejects missing and forged sessions on protected reads and writes", async () => {
    for (const [path, method] of [["/transactions", "GET"], ["/budgets", "POST"], ["/transactions/search/semantic", "POST"]]) {
      expect((await fetch(`${base}${path}`, { method })).status).toBe(401);
      expect((await fetch(`${base}${path}`, { method, headers: { Authorization: "Bearer forged" } })).status).toBe(401);
    }
  });

  it("rejects expired reads and writes without modifying preview data", async () => {
    const token = await login();
    const headers = { Authorization: `Bearer ${token}`, "Content-Type": "application/json" };
    const original = await (await fetch(`${base}/transactions`, { headers })).json();
    expect((await fetch(`${base}/transactions`, { headers })).status).toBe(200);
    now += 86_400_000;
    expect((await fetch(`${base}/transactions`, { headers })).status).toBe(401);
    expect((await fetch(`${base}/transactions`, {
      method: "POST", headers,
      body: JSON.stringify({ amount: "10.00", kind: "Income", date: "2026-10-12" }),
    })).status).toBe(401);
    const fresh = await login();
    expect(await (await fetch(`${base}/transactions`, { headers: { Authorization: `Bearer ${fresh}` } })).json()).toEqual(original);
  });
});

describe("preview mock API", () => {
  it("stores new transactions and reflects them in budget progress", async () => {
    server = createMockApiServer();
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    const address = server.address();
    const baseUrl = `http://127.0.0.1:${address.port}/api`;

    const createResponse = await authenticatedFetch(`${baseUrl}/transactions`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        amount: "25.00",
        kind: "Expense",
        category: "Dining",
        date: "2026-10-11",
        description: "Lunch",
      }),
    });

    expect(createResponse.status).toBe(204);

    const transactions = await authenticatedFetch(`${baseUrl}/transactions`).then((response) => response.json());
    expect(transactions).toContainEqual(expect.objectContaining({
      amount: "-25.00",
      kind: "Expense",
      category: "Dining",
      description: "Lunch",
    }));

    const progress = await authenticatedFetch(`${baseUrl}/budgets/progress?month=2026-10-01`)
      .then((response) => response.json());
    expect(progress).toContainEqual(expect.objectContaining({
      category: "Dining",
      spent: "143.20",
      remaining: "106.80",
    }));
  });
});

describe("preview transaction editing", () => {
  let server;
  let base;

  beforeEach(async () => {
    server = createMockApiServer();
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    base = `http://127.0.0.1:${server.address().port}/api`;
  });

  afterEach(async () => {
    await new Promise((resolve) => server.close(resolve));
  });

  it("updates one existing row and recalculates budget progress", async () => {
    const original = await (await authenticatedFetch(`${base}/transactions`)).json();
    const transaction = original.find((row) => row.category === "Dining");
    const input = { amount: "50.00", kind: "Expense", category: "Groceries", date: "2026-10-12", description: "Corrected purchase" };
    const response = await authenticatedFetch(`${base}/transactions/${transaction.id}`, {
      method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify(input),
    });
    expect(response.status).toBe(200);
    const updated = await (await authenticatedFetch(`${base}/transactions`)).json();
    expect(updated).toHaveLength(original.length);
    expect(updated.find((row) => row.id === transaction.id)).toEqual({ id: transaction.id, ...input, amount: "-50.00", can_revert: false });
    const progress = await (await authenticatedFetch(`${base}/budgets/progress?month=2026-10-01`)).json();
    expect(progress.find((row) => row.category === "Dining").spent).toBe("0.00");
    expect(progress.find((row) => row.category === "Groceries").spent).toBe("200.55");
  });

  it("rejects invalid edits and unknown IDs without changing existing rows", async () => {
    const original = await (await authenticatedFetch(`${base}/transactions`)).json();
    const input = { ...original[0], amount: "0" };
    const options = { method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify(input) };
    expect((await authenticatedFetch(`${base}/transactions/${original[0].id}`, options)).status).toBe(400);
    expect((await authenticatedFetch(`${base}/transactions/missing`, options)).status).toBe(404);
    expect(await (await authenticatedFetch(`${base}/transactions`)).json()).toEqual(original);
  });

  it("starts from a supplied preview snapshot", async () => {
    await new Promise((resolve) => server.close(resolve));
    const transactions = [{ id: "snapshot-row", amount: "15.00", kind: "Income", category: null, date: "2026-10-12", description: "Saved preview data" }];
    server = createMockApiServer({ transactions, budgets: [] });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    base = `http://127.0.0.1:${server.address().port}/api`;
    expect(await (await authenticatedFetch(`${base}/transactions`)).json()).toEqual(transactions.map((transaction) => ({ ...transaction, can_revert: false })));
    expect(await (await authenticatedFetch(`${base}/budgets`)).json()).toEqual([]);
  });
});


describe("preview imported transaction revert", () => {
  it("restores only the supplied import fields and derives eligibility from the immutable baseline", async () => {
    const transaction = { id: "imported", amount: "75.50", kind: "Income", category: "Edited", date: "2026-10-12", description: "Edited import" };
    const original = { kind: "Expense", category: null, description: "Original import" };
    const importedDetails = { imported: original };
    server = createMockApiServer({ transactions: [transaction], budgets: [], importedDetails });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    const base = `http://127.0.0.1:${server.address().port}/api`;
    expect((await (await authenticatedFetch(`${base}/transactions`)).json())[0].can_revert).toBe(true);
    const response = await authenticatedFetch(`${base}/transactions/imported/revert`, { method: "POST" });
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual(original);
    expect(await (await authenticatedFetch(`${base}/transactions`)).json()).toEqual([{ ...transaction, ...original, can_revert: false }]);
    const edit = await authenticatedFetch(`${base}/transactions/imported`, {
      method: "PUT", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ ...transaction, description: "Second edit" }),
    });
    expect(edit.status).toBe(200);
    expect((await (await authenticatedFetch(`${base}/transactions`)).json())[0].can_revert).toBe(true);
    expect(await (await authenticatedFetch(`${base}/transactions/imported/revert`, { method: "POST" })).json()).toEqual(original);
    expect(importedDetails).toEqual({ imported: original });
  });

  it("does not fabricate an original for a manual transaction", async () => {
    const transaction = { id: "manual", amount: "10.00", kind: "Income", category: null, date: "2026-10-12", description: "Manual" };
    server = createMockApiServer({ transactions: [transaction], budgets: [] });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    const base = `http://127.0.0.1:${server.address().port}/api`;
    expect((await authenticatedFetch(`${base}/transactions/manual/revert`, { method: "POST" })).status).toBe(404);
    expect(await (await authenticatedFetch(`${base}/transactions`)).json()).toEqual([{ ...transaction, can_revert: false }]);
  });
});
