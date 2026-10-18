export type TransactionKind = "Income" | "Expense" | "Transfer";


export type BankLinkToken = {
  link_token: string;
  expiration: string;
};

export type BankConnectionResponse = {
  connection_id: string;
  provider: string;
  status: string;
};

export type BankImportResponse = {
  imported: number;
};

export type Transaction = {
  id: string;
  row_version: number;
  amount: string; // Decimal from backend
  kind: TransactionKind;
  category: string | null;
  date: string; // "YYYY-MM-DD"
  description: string | null;
  can_revert?: boolean;
};

export type IndexingStatus = {
  total: number;
  indexed: number;
  remaining: number;
};

export type TransactionInput = Omit<Transaction, "id" | "can_revert" | "row_version">;

export type UpdateTransactionInput = TransactionInput & { expected_version: number };

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
