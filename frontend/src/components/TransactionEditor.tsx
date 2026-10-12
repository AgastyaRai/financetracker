import { useState } from "react";
import type { Transaction, TransactionInput, TransactionKind } from "../types";
import StatusMessageView, { type StatusMessage } from "./StatusMessageView";

type Props = {
  transaction: Transaction;
  submitting: boolean;
  status: StatusMessage | null;
  onSave: (input: TransactionInput) => Promise<void>;
  onCancel: () => void;
};

export default function TransactionEditor({ transaction, submitting, status, onSave, onCancel }: Props) {
  const [amount, setAmount] = useState(() => transaction.amount.replace(/^-/, ""));
  const [kind, setKind] = useState<TransactionKind>(transaction.kind);
  const [date, setDate] = useState(transaction.date);
  const [category, setCategory] = useState(transaction.category ?? "");
  const [description, setDescription] = useState(transaction.description ?? "");

  return (
    <div className="transactionEditor">
      <h3 id="transaction-edit-title">Edit transaction</h3>
      <form
        aria-labelledby="transaction-edit-title"
        className="formGrid transactionForm"
        onSubmit={(event) => {
          event.preventDefault();
          void onSave({
            amount, kind, date,
            category: category.trim() || null,
            description: description.trim() || null,
          });
        }}
      >
        <label htmlFor="edit-transaction-amount">Amount</label>
        <input
          id="edit-transaction-amount" type="number" inputMode="decimal"
          min="0.01" step="0.01" required autoFocus disabled={submitting}
          value={amount} onChange={(event) => setAmount(event.target.value)}
        />

        <label htmlFor="edit-transaction-kind">Kind</label>
        <select
          id="edit-transaction-kind" disabled={submitting}
          value={kind} onChange={(event) => setKind(event.target.value as TransactionKind)}
        >
          <option value="Expense">Expense</option>
          <option value="Income">Income</option>
        </select>

        <label htmlFor="edit-transaction-date">Date</label>
        <input
          id="edit-transaction-date" type="date" required disabled={submitting}
          value={date} onChange={(event) => setDate(event.target.value)}
        />

        <label htmlFor="edit-transaction-category">Category (optional)</label>
        <input
          id="edit-transaction-category" disabled={submitting}
          value={category} onChange={(event) => setCategory(event.target.value)}
        />

        <label htmlFor="edit-transaction-description">Description (optional)</label>
        <input
          id="edit-transaction-description" disabled={submitting}
          value={description} onChange={(event) => setDescription(event.target.value)}
        />

        <div className="fullRow formActions">
          <div className="transactionEditActions">
            <button type="submit" disabled={submitting}>{submitting ? "Saving…" : "Save changes"}</button>
            <button type="button" className="buttonSecondary" disabled={submitting} onClick={onCancel}>Cancel</button>
          </div>
          <StatusMessageView message={status} />
        </div>
      </form>
    </div>
  );
}
