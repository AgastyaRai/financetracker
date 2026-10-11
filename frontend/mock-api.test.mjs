import { afterEach, describe, expect, it } from "vitest";
import { createMockApiServer } from "./mock-api.mjs";

let server;

afterEach(async () => {
  if (!server) return;
  await new Promise((resolve, reject) => {
    server.close((error) => error ? reject(error) : resolve());
  });
  server = undefined;
});

describe("preview mock API", () => {
  it("stores new transactions and reflects them in budget progress", async () => {
    server = createMockApiServer();
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    const address = server.address();
    const baseUrl = `http://127.0.0.1:${address.port}/api`;

    const createResponse = await fetch(`${baseUrl}/transactions`, {
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

    const transactions = await fetch(`${baseUrl}/transactions`).then((response) => response.json());
    expect(transactions).toContainEqual(expect.objectContaining({
      amount: "-25.00",
      kind: "Expense",
      category: "Dining",
      description: "Lunch",
    }));

    const progress = await fetch(`${baseUrl}/budgets/progress?month=2026-10-01`)
      .then((response) => response.json());
    expect(progress).toContainEqual(expect.objectContaining({
      category: "Dining",
      spent: "143.20",
      remaining: "106.80",
    }));
  });
});
