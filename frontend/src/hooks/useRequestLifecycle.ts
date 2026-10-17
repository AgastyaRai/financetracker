import { useCallback, useRef } from "react";

export function useRequestLifecycle() {
  const requestVersions = useRef({ session: 0, transactions: 0, budgets: 0, search: 0 });
  // Responses belong to the session and request that started them, even after logout or Clear.
  const captureSession = useCallback(() => {
    const session = requestVersions.current.session;
    const token = localStorage.getItem("access_token");
    return () => session === requestVersions.current.session
      && token === localStorage.getItem("access_token");
  }, []);

  const beginRequest = useCallback((kind: "transactions" | "budgets" | "search") => {
    const version = ++requestVersions.current[kind];
    const isCurrentSession = captureSession();
    return () => isCurrentSession() && version === requestVersions.current[kind];
  }, [captureSession]);

  return { requestVersions, captureSession, beginRequest };
}
