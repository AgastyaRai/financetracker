import { randomUUID } from "node:crypto";
import { createServer } from "node:http";
import { pathToFileURL } from "node:url";

const seededTransactions = [
  { id: "11111111-1111-1111-1111-111111111111", amount: "4200.00", kind: "Income", category: "Salary", date: "2026-10-01", description: "Monthly salary" },
  { id: "22222222-2222-2222-2222-222222222222", amount: "-86.40", kind: "Expense", category: "Groceries", date: "2026-10-03", description: "Weekend groceries" },
  { id: "33333333-3333-3333-3333-333333333333", amount: "-42.75", kind: "Expense", category: "Transportation", date: "2026-10-05", description: "Train and rideshare" },
  { id: "44444444-4444-4444-4444-444444444444", amount: "-118.20", kind: "Expense", category: "Dining", date: "2026-10-07", description: "Dinner with friends" },
  { id: "55555555-5555-5555-5555-555555555555", amount: "-64.15", kind: "Expense", category: "Groceries", date: "2026-10-09", description: "Fresh market" },
];

const seededBudgets = [
  { month: "2026-10-01", category: "Dining", amount: "250.00" },
  { month: "2026-10-01", category: "Groceries", amount: "500.00" },
  { month: "2026-10-01", category: "Transportation", amount: "200.00" },
];

function sendJson(response, status, body) {
  response.statusCode = status;
  response.setHeader("Content-Type", "application/json");
  response.end(JSON.stringify(body));
}

function readJson(request) {
  return new Promise((resolve, reject) => {
    let body = "";
    request.setEncoding("utf8");
    request.on("data", (chunk) => {
      body += chunk;
    });
    request.on("end", () => {
      try {
        resolve(body ? JSON.parse(body) : {});
      } catch (error) {
        reject(error);
      }
    });
    request.on("error", reject);
  });
}

function budgetProgress(budgets, transactions, month) {
  const monthPrefix = month.slice(0, 7);
  return budgets
    .filter((budget) => budget.month === month)
    .map((budget) => {
      const spent = transactions
        .filter((transaction) =>
          transaction.kind === "Expense"
          && transaction.category === budget.category
          && transaction.date.startsWith(monthPrefix),
        )
        .reduce((total, transaction) => total + Math.abs(Number(transaction.amount)), 0);
      const budgetAmount = Number(budget.amount);

      return {
        category: budget.category,
        budget_amount: budgetAmount.toFixed(2),
        spent: spent.toFixed(2),
        remaining: (budgetAmount - spent).toFixed(2),
      };
    });
}

export function createMockApiServer() {
  const transactions = structuredClone(seededTransactions);
  const budgets = structuredClone(seededBudgets);
  const token = `x.${Buffer.from(JSON.stringify({ exp: Math.floor(Date.now() / 1000) + 86400 })).toString("base64url")}.x`;

  return createServer(async (request, response) => {
    const url = new URL(request.url ?? "/", "http://localhost");

    try {
      if (request.method === "POST" && url.pathname === "/api/users/login") {
        sendJson(response, 200, {
          user_id: "99999999-9999-9999-9999-999999999999",
          access_token: token,
        });
        return;
      }

      if (request.method === "POST" && url.pathname === "/api/users/register") {
        response.statusCode = 204;
        response.end();
        return;
      }

      if (request.method === "GET" && url.pathname === "/api/transactions") {
        sendJson(response, 200, transactions);
        return;
      }

      if (request.method === "POST" && url.pathname === "/api/transactions") {
        const input = await readJson(request);
        const amount = Number(input.amount);
        if (!Number.isFinite(amount) || amount <= 0 || !["Income", "Expense"].includes(input.kind)) {
          sendJson(response, 400, { error: "Invalid transaction" });
          return;
        }

        transactions.push({
          id: randomUUID(),
          amount: (input.kind === "Expense" ? -amount : amount).toFixed(2),
          kind: input.kind,
          category: input.category ?? null,
          date: input.date,
          description: input.description ?? null,
        });
        response.statusCode = 204;
        response.end();
        return;
      }

      if (request.method === "GET" && url.pathname === "/api/budgets") {
        const month = url.searchParams.get("month");
        sendJson(response, 200, month ? budgets.filter((budget) => budget.month === month) : budgets);
        return;
      }

      if (request.method === "POST" && url.pathname === "/api/budgets") {
        const input = await readJson(request);
        const existing = budgets.find((budget) =>
          budget.month === input.month && budget.category === input.category,
        );
        if (existing) existing.amount = input.amount;
        else budgets.push(input);
        response.statusCode = 204;
        response.end();
        return;
      }

      if (request.method === "GET" && url.pathname === "/api/budgets/progress") {
        const month = url.searchParams.get("month") ?? "2026-10-01";
        sendJson(response, 200, budgetProgress(budgets, transactions, month));
        return;
      }

      if (request.method === "POST" && url.pathname === "/api/transactions/search/semantic") {
        sendJson(response, 200, { transactions: [], summary: null });
        return;
      }

      sendJson(response, 404, { error: "Not found" });
    } catch {
      sendJson(response, 400, { error: "Invalid request" });
    }
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const port = Number(process.env.PORT ?? 3000);
  createMockApiServer().listen(port, "127.0.0.1", () => {
    console.log(`Stateful preview API listening on http://127.0.0.1:${port}`);
  });
}
