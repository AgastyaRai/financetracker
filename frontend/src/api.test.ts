import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as indexingApi from "./api";
import { getTransactions, loginUser, setUnauthorizedCallback, updateTransaction, revertTransaction } from "./api";

describe("API requests", () => {
  const fetchMock = vi.fn();

  beforeEach(() => {
    fetchMock.mockReset();
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    setUnauthorizedCallback(null);
  });

  it("includes the access token in authenticated requests", async () => {
    localStorage.setItem("access_token", "test-token");
    fetchMock.mockResolvedValue({
      status: 200,
      ok: true,
      text: vi.fn().mockResolvedValue("[]"),
    });

    await getTransactions();

    expect(fetchMock).toHaveBeenCalledWith(
      "/api/transactions",
      expect.objectContaining({
        method: "GET",
        headers: expect.objectContaining({
          Authorization: "Bearer test-token",
          "Content-Type": "application/json",
        }),
      }),
    );
  });

  it("clears authentication and calls the logout handler after a 401", async () => {
    const logout = vi.fn();
    localStorage.setItem("access_token", "expired-token");
    localStorage.setItem("user_id", "test-user");
    setUnauthorizedCallback(logout);
    fetchMock.mockResolvedValue({
      status: 401,
      ok: false,
      text: vi.fn().mockResolvedValue(""),
    });

    await expect(getTransactions()).rejects.toThrow("Session expired. Please log in again.");

    expect(localStorage.getItem("access_token")).toBeNull();
    expect(localStorage.getItem("user_id")).toBeNull();
    expect(logout).toHaveBeenCalledOnce();
  });

  it("extracts readable messages from JSON API errors", async () => {
    fetchMock.mockResolvedValue({
      status: 400,
      ok: false,
      text: vi.fn().mockResolvedValue(JSON.stringify({ error: "Invalid transaction" })),
    });

    await expect(getTransactions()).rejects.toMatchObject({ message: "Invalid transaction" });
  });

  it("preserves a newer session when an old request returns 401", async () => {
    const logout = vi.fn();
    let respond!: (response: Response) => void;
    fetchMock.mockReturnValue(new Promise<Response>((resolve) => { respond = resolve; }));
    setUnauthorizedCallback(logout);
    localStorage.setItem("access_token", "old-token");
    const request = getTransactions();
    localStorage.setItem("access_token", "new-token");
    respond(new Response("", { status: 401 }));
    await expect(request).rejects.toThrow("Session expired. Please log in again.");
    expect(localStorage.getItem("access_token")).toBe("new-token");
    expect(logout).not.toHaveBeenCalled();
  });

  it("shows a failed login's credential error without expiring a stored session", async () => {
    const logout = vi.fn();
    setUnauthorizedCallback(logout);
    localStorage.setItem("access_token", "other-tab-token");
    fetchMock.mockResolvedValue(new Response("Invalid username/email or password", { status: 401 }));
    await expect(loginUser({ identifier: "user", password: "wrong-password" })).rejects.toThrow(
      "Invalid username/email or password",
    );
    expect(localStorage.getItem("access_token")).toBe("other-tab-token");
    expect(logout).not.toHaveBeenCalled();
    expect(fetchMock.mock.calls[0][1].headers).not.toHaveProperty("Authorization");
  });
});


describe("transaction updates", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("sends authenticated PUT fields and accepts an empty success response", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(null, { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);
    localStorage.setItem("access_token", "edit-token");
    const input = { expected_version: 1, amount: "40.50", kind: "Expense" as const, category: null, date: "2026-10-12", description: "Coffee" };
    await expect(updateTransaction("transaction-id", input)).resolves.toBeUndefined();
    expect(fetchMock).toHaveBeenCalledWith("/api/transactions/transaction-id", expect.objectContaining({
      method: "PUT", body: JSON.stringify(input),
      headers: expect.objectContaining({ Authorization: "Bearer edit-token" }),
    }));
  });
});


describe("imported transaction revert request", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("posts only the transaction id and receives the original fields", async () => {
    const original = { kind: "Expense", category: null, description: "Original" };
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify(original), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);
    localStorage.setItem("access_token", "revert-token");
    await expect(revertTransaction("imported/id")).resolves.toEqual(original);
    expect(fetchMock).toHaveBeenCalledWith("/api/transactions/imported%2Fid/revert", expect.objectContaining({
      method: "POST", headers: expect.objectContaining({ Authorization: "Bearer revert-token" }),
    }));
    expect(fetchMock.mock.calls[0][1]).not.toHaveProperty("body");
  });
});


describe("indexing progress API", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    setUnauthorizedCallback(null);
  });

  it("reads account-wide counts with authentication and supports cancellation", async () => {
    // Return the server's counts unchanged; the client must not count visible rows.
    const counts = { total: 331, indexed: 172, remaining: 159 };
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify(counts), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);
    localStorage.setItem("access_token", "indexing-token");
    const controller = new AbortController();

    expect(indexingApi.getIndexingStatus).toBeTypeOf("function");
    await expect(indexingApi.getIndexingStatus(controller.signal)).resolves.toEqual(counts);
    expect(fetchMock).toHaveBeenCalledExactlyOnceWith("/api/transactions/indexing-status", expect.objectContaining({
      method: "GET", signal: controller.signal,
      headers: expect.objectContaining({ Authorization: "Bearer indexing-token" }),
    }));
    expect(fetchMock.mock.calls[0][1]).not.toHaveProperty("body");
  });

  it("uses the existing session-expired handling for an indexing-status 401", async () => {
    const logout = vi.fn();
    setUnauthorizedCallback(logout);
    localStorage.setItem("access_token", "expired-indexing-token");
    localStorage.setItem("user_id", "old-user");
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response("", { status: 401 })));

    expect(indexingApi.getIndexingStatus).toBeTypeOf("function");
    await expect(indexingApi.getIndexingStatus()).rejects.toThrow("Session expired. Please log in again.");
    expect(logout).toHaveBeenCalledOnce();
    expect(localStorage.getItem("access_token")).toBeNull();
    expect(localStorage.getItem("user_id")).toBeNull();
  });
});
