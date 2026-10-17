import { useEffect, useState } from "react";
import { getIndexingStatus } from "../api";
import type { IndexingStatus } from "../types";

export default function IndexingProgress() {
  const [status, setStatus] = useState<IndexingStatus | null>(null);
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    const token = localStorage.getItem("access_token");
    const controller = new AbortController();
    let active = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const isCurrent = () => active && token !== null && token === localStorage.getItem("access_token");

    async function loadStatus() {
      if (!isCurrent()) return;
      try {
        const counts = await getIndexingStatus(controller.signal);
        if (!isCurrent()) return;
        setStatus(counts);
        // Start the delay after the response so slow requests cannot overlap polls.
        if (counts.remaining > 0) timer = setTimeout(() => { void loadStatus(); }, 5_000);
      } catch {
        if (!isCurrent()) return;
        setStatus(null);
        setFailed(true);
      }
    }

    void loadStatus();
    return () => {
      // Abort transport work and ignore completions even if a provider does not honor abort.
      active = false;
      clearTimeout(timer);
      controller.abort();
    };
  }, [attempt]);

  function retry() {
    setFailed(false);
    setStatus(null);
    setAttempt((value) => value + 1);
  }

  return (
    <section aria-label="Search indexing progress" aria-live="polite" aria-atomic="true">
      {failed ? (
        <div>
          <p className="status error">Could not load search indexing progress.</p>
          <button type="button" className="buttonSecondary" aria-label="Retry indexing progress" onClick={retry}>Retry</button>
        </div>
      ) : (
        <p className="muted">
          {status === null ? "Loading search indexing progress…"
            : status.total === 0 ? "No transactions to index"
            : status.remaining === 0 ? `All ${status.total} transactions ready for search`
            : `Preparing search: ${status.indexed} of ${status.total} transactions ready, ${status.remaining} remaining.`}
        </p>
      )}
    </section>
  );
}
