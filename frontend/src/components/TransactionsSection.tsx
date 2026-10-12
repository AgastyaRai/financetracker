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
  const editButtonRef = useRef<HTMLButtonElement | null>(null);
  const queryInputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    if (editingTransaction || !editButtonRef.current) return;
    if (editButtonRef.current.isConnected && !editButtonRef.current.disabled) editButtonRef.current.focus();
    else queryInputRef.current?.focus();
    editButtonRef.current = null;
  }, [editingTransaction]);

  function editButton(transaction: Transaction) {
    return (
      <button
        type="button" className="buttonSecondary transactionEditButton" disabled={submittingEdit}
        aria-label={`Edit transaction: ${transaction.description || transaction.category || transaction.kind}, ${transaction.date}`}
        aria-pressed={editingTransaction?.id === transaction.id}
        onClick={(event) => {
          editButtonRef.current = event.currentTarget;
          startEditingTransaction(transaction);
        }}
      >Edit</button>
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

      {editingTransaction && (
        <TransactionEditor
          key={editingTransaction.id} transaction={editingTransaction}
          submitting={submittingEdit} status={editStatus}
          onSave={handleUpdateTransaction} onCancel={cancelEditingTransaction}
        />
      )}

      <StatusMessageView message={transactionsStatus} />

      {semanticResults?.summary && (
        <div className="card" style={{ marginBottom: 14 }}>
          <h3>AI Summary</h3>
          <p>{semanticResults.summary}</p>
        </div>
      )}

      {shownSemanticTransactions ? (
        shownSemanticTransactions.length === 0 ? (
          <p className="muted">No matching transactions found.</p>
        ) : (
          <div className="tableScroll transactionTable" tabIndex={0} role="region" aria-label="Transactions table">
            <table>
              <thead>
                <tr>
                  <th>Date</th>
                  <th>Kind</th>
                  <th>Category</th>
                  <th>Description</th>
                  <th>Similarity</th>
                  <th style={{ textAlign: "right" }}>Amount</th>
                  <th>Actions</th>
                </tr>
              </thead>
              <tbody>
                {shownSemanticTransactions.map((match) => (
                  <tr key={match.transaction.id}>
                    <td>{match.transaction.date}</td>
                    <td>{match.transaction.kind}</td>
                    <td>{match.transaction.category ?? "-"}</td>
                    <td>{match.transaction.description ?? "-"}</td>
                    <td>{(match.similarity_score * 100).toFixed(1)}%</td>
                    <td style={{ textAlign: "right" }}>
                      {money(Number(match.transaction.amount))}
                    </td>
                    <td>{editButton(match.transaction)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )
      ) : loadingTx ? (
        <p className="muted">Loading…</p>
      ) : transactions.length === 0 ? (
        <p className="muted">No transactions yet.</p>
      ) : (
        <div className="tableScroll transactionTable" tabIndex={0} role="region" aria-label="Transactions table">
          <table>
            <thead>
              <tr>
                <th>Date</th>
                <th>Kind</th>
                <th>Category</th>
                <th>Description</th>
                <th style={{ textAlign: "right" }}>Amount</th>
                <th>Actions</th>
              </tr>
            </thead>
            <tbody>
              {transactions.map((t) => (
                <tr key={t.id}>
                  <td>{t.date}</td>
                  <td>{t.kind}</td>
                  <td>{t.category ?? "-"}</td>
                  <td>{t.description ?? "-"}</td>
                  <td style={{ textAlign: "right" }}>{money(Number(t.amount))}</td>
                  <td>{editButton(t)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
