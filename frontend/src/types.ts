export type TransactionKind = "Income" | "Expense";

export type Transaction = {
  user_id: string;
  amount: string; // Decimal from backend
  kind: TransactionKind;
  category: string | null;
  date: string; // "YYYY-MM-DD"
  description: string | null;
};

export type LoginResponse = { user_id: string };

export type Budget = {
  user_id: string;
  month: string; // "YYYY-MM-01"
  category: string;
  amount: string; // Decimal as string
};

export type BudgetProgress = {
  category: string;
  budget_amount: string;
  spent: string;
  remaining: string;
};
