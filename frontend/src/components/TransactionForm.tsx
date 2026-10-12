import type { TransactionKind } from "../types";
import StatusMessageView, { type StatusMessage } from "./StatusMessageView";

type Props = {
  amount: string;
  setAmount: (value: string) => void;
  kind: TransactionKind;
  setKind: (value: TransactionKind) => void;
  date: string;
  setDate: (value: string) => void;
  category: string;
  setCategory: (value: string) => void;
  description: string;
  setDescription: (value: string) => void;
  submittingTransaction: boolean;
  transactionStatus: StatusMessage | null;
  handleAddTransaction: () => Promise<void>;
};

export default function TransactionForm({
  amount, setAmount, kind, setKind, date, setDate, category, setCategory, description, setDescription, submittingTransaction, transactionStatus, handleAddTransaction
}: Props) {
  return (
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
          <button type="submit" disabled={submittingTransaction}>
            {submittingTransaction ? "Adding…" : "Add"}
          </button>
          <StatusMessageView message={transactionStatus} />
        </div>
      </form>
    </div>
  );
}
