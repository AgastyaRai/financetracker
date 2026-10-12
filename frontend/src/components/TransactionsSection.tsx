import { useEffect, useRef } from "react";
import TransactionEditor from "./TransactionEditor";
import type { Transaction, TransactionInput, SemanticSearchResult } from "../types";
import { money } from "../format";
import StatusMessageView, { type StatusMessage } from "./StatusMessageView";

type Props = {
  semanticQuery: string;
  setSemanticQuery: (value: string) => void;
  semanticLimit: string;
  setSemanticLimit: (value: string) => void;
  includeSummary: boolean;
  setIncludeSummary: (value: boolean) => void;
  searchingSemantic: boolean;
  handleSemanticSearch: () => Promise<void>;
  clearSemanticSearch: () => void;
  transactionsStatus: StatusMessage | null;
  semanticResults: SemanticSearchResult | null;
  loadingTx: boolean;
  transactions: Transaction[];
  editingTransaction: Transaction | null;
  submittingEdit: boolean;
  editStatus: StatusMessage | null;
  startEditingTransaction: (transaction: Transaction) => void;
  cancelEditingTransaction: () => void;
  handleUpdateTransaction: (input: TransactionInput) => Promise<void>;
};

export default function TransactionsSection({
  semanticQuery, setSemanticQuery, semanticLimit, setSemanticLimit, includeSummary, setIncludeSummary, searchingSemantic, handleSemanticSearch, clearSemanticSearch, transactionsStatus, semanticResults, loadingTx, transactions, editingTransaction, submittingEdit, editStatus, startEditingTransaction, cancelEditingTransaction, handleUpdateTransaction
}: Props) {
  const shownSemanticTransactions = semanticResults?.transactions ?? null;
  const shownRows: Array<{ transaction: Transaction; similarity?: number }> = shownSemanticTransactions
    ? shownSemanticTransactions.map((match) => ({ transaction: match.transaction, similarity: match.similarity_score }))
    : transactions.map((transaction) => ({ transaction }));
  const rows = editingTransaction && !shownRows.some((row) => row.transaction.id === editingTransaction.id)
    ? [{ transaction: editingTransaction }, ...shownRows] : shownRows;
  const editButtonRef = useRef<string | null>(null);
  const rowActionRefs = useRef(new Map<string, HTMLButtonElement>());
  const queryInputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    if (editingTransaction || !editButtonRef.current) return;
    const button = rowActionRefs.current.get(editButtonRef.current);
    if (button?.isConnected && !button.disabled) button.focus();
    else queryInputRef.current?.focus();
    editButtonRef.current = null;
  }, [editingTransaction]);

  function editButton(transaction: Transaction) {
    return (
      <button
        ref={(button) => {
          if (button) rowActionRefs.current.set(transaction.id, button);
          else rowActionRefs.current.delete(transaction.id);
        }}
        type="button" className="transactionRowAction" disabled={submittingEdit} title="Edit transaction"
        aria-label={`Edit transaction: ${transaction.description || transaction.category || transaction.kind}, ${transaction.date}`}
        onClick={() => {
          editButtonRef.current = transaction.id;
          startEditingTransaction(transaction);
        }}
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d="m16 4 4 4-11 11H5v-4L16 4Z" />
          <path d="m14 6 4 4" />
        </svg>
      </button>
    );
  }

  return (
    <div className="card">
      <h2>Transactions</h2>

      <form
        className="transactionSearch"
        onSubmit={(e) => { e.preventDefault(); void handleSemanticSearch(); }}
      >
        <input
          ref={queryInputRef}
          aria-label="Semantic search query"
          value={semanticQuery}
          onChange={(e) => setSemanticQuery(e.target.value)}
          maxLength={500}
          placeholder="Search transactions semantically (e.g. uber, groceries, ride home)"
          className="searchQuery"
        />

        <input
          aria-label="Maximum search results"
          type="number"
          min={1}
          max={50}
          value={semanticLimit}
          onChange={(e) => setSemanticLimit(e.target.value)}
          style={{ width: 90 }}
        />

        <label style={{ display: "flex", alignItems: "center", gap: 6 }}>
          <input
            type="checkbox"
            checked={includeSummary}
            onChange={(e) => setIncludeSummary(e.target.checked)}
          />
          Summarize with AI
        </label>

        <div className="searchActions">
          <button type="submit" disabled={searchingSemantic || submittingEdit}>
            {searchingSemantic ? "Searching..." : "Search"}
          </button>

          <button type="button" className="buttonSecondary" onClick={clearSemanticSearch}>
            Clear
          </button>
        </div>
      </form>

      <StatusMessageView message={transactionsStatus} />
      {editingTransaction && editStatus && (
        <div id="transaction-edit-error"><StatusMessageView message={editStatus} /></div>
      )}

      {semanticResults?.summary && (
        <div className="card" style={{ marginBottom: 14 }}>
          <h3>AI Summary</h3>
          <p>{semanticResults.summary}</p>
        </div>
      )}

      {loadingTx && !shownSemanticTransactions && !editingTransaction ? (
        <p className="muted">Loading…</p>
      ) : rows.length === 0 ? (
        <p className="muted">{shownSemanticTransactions ? "No matching transactions found." : "No transactions yet."}</p>
      ) : (
        <div className={`tableScroll transactionTable${shownSemanticTransactions ? " transactionSemanticTable" : ""}`} tabIndex={0} role="region" aria-label="Transactions table">
          <table>
            <colgroup>
              <col className="transactionDateColumn" />
              <col className="transactionKindColumn" />
              <col className="transactionCategoryColumn" />
              <col />
              {shownSemanticTransactions && <col className="transactionSimilarityColumn" />}
              <col className="transactionAmountColumn" />
              <col className="transactionActionsColumn" />
            </colgroup>
            <thead>
              <tr>
                <th>Date</th>
                <th>Kind</th>
                <th>Category</th>
                <th>Description</th>
                {shownSemanticTransactions && <th>Similarity</th>}
                <th style={{ textAlign: "right" }}>Amount</th>
                <th>Actions</th>
              </tr>
            </thead>
            <tbody>
              {rows.map(({ transaction: t, similarity }) => editingTransaction?.id === t.id ? (
                <TransactionEditor
                  key={t.id} transaction={editingTransaction} similarity={similarity}
                  showSimilarity={shownSemanticTransactions !== null}
                  submitting={submittingEdit} status={editStatus}
                  onSave={handleUpdateTransaction} onCancel={cancelEditingTransaction}
                />
              ) : (
                <tr key={t.id}>
                  <td>{t.date}</td>
                  <td>{t.kind}</td>
                  <td>{t.category ?? "-"}</td>
                  <td>{t.description ?? "-"}</td>
                  {shownSemanticTransactions && <td>{similarity === undefined ? "-" : `${(similarity * 100).toFixed(1)}%`}</td>}
                  <td style={{ textAlign: "right" }}>{money(Number(t.amount))}</td>
                  <td className="transactionRowActions">{editButton(t)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
