export type TransactionKind = "Income" | "Expense";

export type Transaction = {
  id: string;
  amount: string; // Decimal from backend
  kind: TransactionKind;
  category: string | null;
  date: string; // "YYYY-MM-DD"
  description: string | null;
};

export type TransactionInput = Omit<Transaction, "id">;

export type LoginResponse = { 
  user_id: string;
  access_token: string;
};

export type Budget = {
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

export type SemanticTransaction = {
  transaction: Transaction;
  similarity_score: number;
};

export type SemanticSearchResult = {
  transactions: SemanticTransaction[];
  summary: string | null;
};
