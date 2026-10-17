import { useEffect, useRef, useState } from "react";
import { usePlaidLink } from "react-plaid-link";
import type { PlaidLinkOnExit, PlaidLinkOnSuccess } from "react-plaid-link";
import { createBankLinkToken, exchangeBankToken, getBankConnections, importBankTransactions } from "./api";
import type { BankConnectionResponse } from "./types";

type Phase = "idle" | "requesting" | "linking" | "exchanging" | "importing";
type Message = { text: string; type: "error" | "success" | "info" };
type Props = { onImported: (isCurrent: () => boolean) => Promise<void> };

const connectionStatusLabels: Record<string, string> = {
  active: "Active",
  needs_reconnection: "Needs reconnection",
  disconnected: "Disconnected",
};

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

// mount the SDK only for an active Link attempt; unmounting lets the SDK close and destroy its window
function PlaidWindow({ token, onSuccess, onExit, onUnavailable }: {
  token: string;
  onSuccess: PlaidLinkOnSuccess;
  onExit: PlaidLinkOnExit;
  onUnavailable: () => void;
}) {
  const { ready, error, open } = usePlaidLink({ token, onSuccess, onExit });
  const opened = useRef(false);

  useEffect(() => {
    if (error) {
      onUnavailable();
      return;
    }
    // the hook can re-render as the script loads; open this particular attempt only once
    if (ready && !opened.current) {
      opened.current = true;
      open();
    }
  }, [ready, error, open, onUnavailable]);

  return null;
}

export default function ConnectBank({ onImported }: Props) {
  const [phase, setPhase] = useState<Phase>("idle");
  const [link, setLink] = useState<{ token: string; attempt: number } | null>(null);
  const [connectionId, setConnectionId] = useState<string | null>(null);
  const [connections, setConnections] = useState<BankConnectionResponse[]>([]);
  const [loadingConnections, setLoadingConnections] = useState(true);
  const [connectionsError, setConnectionsError] = useState<string | null>(null);
  const [connectionLoadAttempt, setConnectionLoadAttempt] = useState(0);
  const [importFailed, setImportFailed] = useState(false);
  const [message, setMessage] = useState<Message | null>(null);
  const phaseRef = useRef<Phase>("idle");
  const attemptRef = useRef(0);
  const mounted = useRef(false);
  const session = useRef(localStorage.getItem("access_token"));
  const refreshDashboard = useRef(onImported);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  // Restore the server's saved list on mount or retry; a reload does not open Plaid or import.
  useEffect(() => {
    let currentRequest = true;
    async function loadConnections() {
      try {
        const saved = await getBankConnections();
        if (!currentRequest || !isCurrent()) return;
        // Linking can finish while this read is pending. Keep new rows and deduplicate by connection ID.
        setConnections((current) => {
          const savedIds = new Set(saved.map((connection) => connection.connection_id));
          return [...saved, ...current.filter((connection) => !savedIds.has(connection.connection_id))];
        });
      } catch (error) {
        if (currentRequest && isCurrent()) {
          setConnectionsError(`Could not load saved connections: ${errorMessage(error)}`);
        }
      } finally {
        if (currentRequest && isCurrent()) setLoadingConnections(false);
      }
    }
    void loadConnections();
    // Ignore this request after unmount or replacement, including Strict Mode's initial effect cleanup.
    return () => { currentRequest = false; };
  }, [connectionLoadAttempt]);

  // Plaid retains the callbacks from initialization; use the latest dashboard callback if the selected month changes
  useEffect(() => { refreshDashboard.current = onImported; }, [onImported]);

  function isCurrent() {
    return mounted.current && session.current !== null && localStorage.getItem("access_token") === session.current;
  }

  function changePhase(next: Phase) {
    // update the guard immediately so two clicks or callbacks in the same render cannot submit twice
    phaseRef.current = next;
    setPhase(next);
  }

  async function importConnection(savedConnectionId: string) {
    if (!isCurrent()) return;
    setConnectionId(savedConnectionId);
    changePhase("importing");
    setImportFailed(false);
    setMessage({ text: "Importing transactions…", type: "info" });

    let imported: number;
    try {
      const result = await importBankTransactions(savedConnectionId);
      if (!isCurrent()) return;
      imported = result.imported;
    } catch (error) {
      if (!isCurrent()) return;
      // linking already saved the connection: retain its ID so retry does not repeat the token exchange
      setImportFailed(true);
      setMessage({ text: `Bank connected, but importing failed: ${errorMessage(error)}`, type: "error" });
      changePhase("idle");
      return;
    }

    const success = imported === 0 ? "Connected; no transactions imported yet."
      : `Imported ${imported} transaction${imported === 1 ? "" : "s"}.`;
    setMessage({ text: success, type: "success" });
    try {
      // the caller checks this same session before applying fetched dashboard data
      await refreshDashboard.current(isCurrent);
    } catch {
      if (isCurrent()) {
        setMessage({ text: `${success} Could not refresh the dashboard. Please refresh the page.`, type: "error" });
      }
    } finally {
      if (isCurrent()) changePhase("idle");
    }
  }

  async function finishLink(publicToken: string | null, attempt: number) {
    if (!isCurrent() || attemptRef.current !== attempt || phaseRef.current !== "linking") return;
    changePhase("exchanging");
    setLink(null);
    setMessage({ text: "Saving bank connection…", type: "info" });

    try {
      if (!publicToken) throw new Error("Plaid did not return a linking token. Please connect again.");
      const saved = await exchangeBankToken(publicToken);
      if (!isCurrent()) return;
      setConnectionId(saved.connection_id);
      setConnections((current) => [
        ...current.filter((connection) => connection.connection_id !== saved.connection_id), saved,
      ]);
      await importConnection(saved.connection_id);
    } catch (error) {
      if (!isCurrent()) return;
      setMessage({ text: errorMessage(error), type: "error" });
      changePhase("idle");
    }
  }

  function closeLink(attempt: number, failed: boolean) {
    if (!isCurrent() || attemptRef.current !== attempt || phaseRef.current !== "linking") return;
    setLink(null);
    changePhase("idle");
    setMessage(failed
      ? { text: "Plaid could not complete bank linking. Please try again.", type: "error" }
      : { text: "Bank connection cancelled.", type: "info" });
  }

  async function startLink() {
    if (!isCurrent() || phaseRef.current !== "idle") return;
    const attempt = ++attemptRef.current;
    changePhase("requesting");
    setMessage({ text: "Preparing bank connection…", type: "info" });

    try {
      const token = await createBankLinkToken();
      if (!isCurrent()) return;
      changePhase("linking");
      setMessage({ text: "Complete the connection in Plaid.", type: "info" });
      setLink({ token: token.link_token, attempt });
    } catch (error) {
      if (!isCurrent()) return;
      setMessage({ text: errorMessage(error), type: "error" });
      changePhase("idle");
    }
  }

  function retryImport(savedConnectionId: string) {
    if (phaseRef.current !== "idle" || !connections.some((connection) =>
      connection.connection_id === savedConnectionId && connection.status === "active")) return;
    void importConnection(savedConnectionId);
  }

  function retryLoadingConnections() {
    if (!isCurrent() || loadingConnections) return;
    setLoadingConnections(true);
    setConnectionsError(null);
    setConnectionLoadAttempt((attempt) => attempt + 1);
  }

  const busy = phase !== "idle";
  return (
    <section className="card dashboardSection" aria-label="Bank connection">
      <div className="sectionHeader">
        <h2>Bank connection</h2>
        <div className="sectionHeaderControls">
          <button type="button" onClick={() => void startLink()} disabled={busy}>Connect bank</button>
        </div>
      </div>
      {loadingConnections ? <p className="muted small">Loading saved connections...</p> : null}
      {connectionsError ? (
        <div>
          <p role="alert" className="status error">{connectionsError}</p>
          <button type="button" className="buttonSecondary" onClick={retryLoadingConnections} disabled={loadingConnections}>
            Retry loading connections
          </button>
        </div>
      ) : null}
      {!loadingConnections && !connectionsError && connections.length === 0 ? (
        <p className="muted small">No saved bank connections.</p>
      ) : null}
      {connections.map((connection, index) => (
        <div key={connection.connection_id} role="group" aria-label={`Plaid connection ${index + 1}`} className="sectionHeader">
          <div>
            <strong>Plaid connection {index + 1}</strong>
            <p className="muted small">{connectionStatusLabels[connection.status]}</p>
          </div>
          {connection.status === "active" ? (
            <button type="button" className="buttonSecondary" onClick={() => retryImport(connection.connection_id)} disabled={busy}>
              {importFailed && connectionId === connection.connection_id ? "Retry import" : "Import again"}
            </button>
          ) : null}
        </div>
      ))}
      {message ? <p role={message.type === "error" ? "alert" : "status"} className={`status ${message.type}`}>{message.text}</p> : null}
      {link ? (
        <PlaidWindow
          key={link.attempt}
          token={link.token}
          onSuccess={(publicToken) => { void finishLink(publicToken, link.attempt); }}
          onExit={(error) => closeLink(link.attempt, error !== null)}
          onUnavailable={() => {
            if (!isCurrent() || attemptRef.current !== link.attempt || phaseRef.current !== "linking") return;
            setLink(null);
            changePhase("idle");
            setMessage({ text: "Could not open Plaid. Please try again.", type: "error" });
          }}
        />
      ) : null}
    </section>
  );
}
