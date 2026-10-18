import { useState } from "react";
import type { Transaction, TransactionInput, TransactionKind } from "../types";
import type { StatusMessage } from "./StatusMessageView";

type Props = {
  transaction: Transaction;
  similarity?: number;
  showSimilarity: boolean;
  submitting: boolean;
  status: StatusMessage | null;
  conflicted: boolean;
  onSave: (input: TransactionInput) => Promise<void>;
  onCancel: () => void;
};

export default function TransactionEditor({ transaction, similarity, showSimilarity, submitting, status, conflicted, onSave, onCancel }: Props) {
  const [amount, setAmount] = useState(() => transaction.amount);
  const [kind, setKind] = useState<TransactionKind>(transaction.kind);
  const [date, setDate] = useState(transaction.date);
  const [category, setCategory] = useState(transaction.category ?? "");
  const [description, setDescription] = useState(transaction.description ?? "");
  const formId = `edit-transaction-${transaction.id}`;
  const errorId = status ? "transaction-edit-error" : undefined;

  return (
    <>
      <tr
        className="transactionEditingRow"
        onFocus={(event) => event.target.scrollIntoView?.({ block: "nearest", inline: "nearest" })}
        onKeyDown={(event) => {
          if (event.key === "Escape" && !submitting) {
            event.preventDefault();
            onCancel();
          }
        }}
      >
        <td>
          <input
            form={formId} aria-label="Date" aria-describedby={errorId}
            type="date" required disabled={submitting}
            value={date} onChange={(event) => setDate(event.target.value)}
          />
        </td>
        <td>
          <select
            form={formId} aria-label="Kind" aria-describedby={errorId} disabled={submitting}
            value={kind} onChange={(event) => setKind(event.target.value as TransactionKind)}
          >
            <option value="Expense">Expense</option>
            <option value="Income">Income</option>
            <option value="Transfer">Transfer</option>
          </select>
        </td>
        <td>
          <input
            form={formId} aria-label="Category (optional)" aria-describedby={errorId} disabled={submitting}
            value={category} onChange={(event) => setCategory(event.target.value)}
          />
        </td>
        <td>
          <input
            form={formId} aria-label="Description (optional)" aria-describedby={errorId} disabled={submitting}
            value={description} onChange={(event) => setDescription(event.target.value)}
          />
        </td>
        {showSimilarity && <td>{similarity === undefined ? "-" : `${(similarity * 100).toFixed(1)}%`}</td>}
        <td>
          <input
            form={formId} aria-label="Amount" aria-describedby={errorId}
            type="number" inputMode="decimal" step="0.01" required autoFocus disabled={submitting}
            value={amount} onChange={(event) => setAmount(event.target.value)}
          />
        </td>
        <td>
          <form
            id={formId} aria-label="Edit transaction" className="transactionEditActions"
            onSubmit={(event) => {
              event.preventDefault();
              if (conflicted || submitting) return;
              void onSave({
                amount, kind, date,
                category: category.trim() || null,
                description: description.trim() || null,
              });
            }}
          >
            <button
              type="submit" className="transactionRowAction transactionRowActionSave" disabled={submitting || conflicted}
              aria-label={submitting && !conflicted ? "Saving…" : "Save changes"} title={submitting && !conflicted ? "Saving…" : "Save changes"}
            >
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path d={submitting ? "M5 12h.01M12 12h.01M19 12h.01" : "m5 12 5 5 9-10"} />
              </svg>
            </button>
            <button
              type="button" className="transactionRowAction transactionRowActionCancel" disabled={submitting}
              aria-label="Cancel" title="Cancel" onClick={onCancel}
            >
              <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m6 6 12 12M18 6 6 18" /></svg>
            </button>
          </form>
        </td>
      </tr>
    </>
  );
}
