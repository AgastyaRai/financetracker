import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";

describe("bank connection API requests", () => {
  const fetchMock = vi.fn();

  beforeEach(() => {
    vi.stubGlobal("fetch", fetchMock);
    fetchMock.mockReset();
    localStorage.setItem("access_token", "finance-session");
  });

  afterEach(() => vi.unstubAllGlobals());

  function respond(body: unknown, status = 200) {
    fetchMock.mockResolvedValue({ status, ok: status < 400, text: async () => JSON.stringify(body) });
  }

  it("loads saved connections using the existing login and returns their metadata", async () => {
    const connections = [
      { connection_id: "saved-first", provider: "plaid", status: "active" },
      { connection_id: "saved-second", provider: "plaid", status: "needs_reconnection" },
    ];
    respond(connections);

    // The list is a read authenticated by the current login, with no IDs or credentials in its body.
    expect(api.getBankConnections).toBeTypeOf("function");
    expect(await api.getBankConnections()).toEqual(connections);
    expect(fetchMock).toHaveBeenCalledExactlyOnceWith("/api/bank-connections", expect.objectContaining({
      method: "GET", headers: expect.objectContaining({ Authorization: "Bearer finance-session" }),
    }));
    expect(fetchMock.mock.calls[0][1].body).toBeUndefined();
  });

  it("logs out through the existing handler when loading saved connections returns 401", async () => {
    const logout = vi.fn();
    api.setUnauthorizedCallback(logout);
    localStorage.setItem("user_id", "previous-user");
    respond("expired", 401);

    // Exercise the actual request wrapper so this checks logout wiring, not a simulated UI callback.
    expect(api.getBankConnections).toBeTypeOf("function");
    await expect(api.getBankConnections()).rejects.toThrow("Session expired. Please log in again.");
    expect(localStorage.getItem("access_token")).toBeNull();
    expect(localStorage.getItem("user_id")).toBeNull();
    expect(logout).toHaveBeenCalledOnce();
  });

  it("requests a Link token using the existing login, with no bank credentials in the request", async () => {
    const token = { link_token: "link-test", expiration: "2026-10-16T20:00:00Z" };
    respond(token);

    expect(await api.createBankLinkToken()).toEqual(token);
    expect(fetchMock).toHaveBeenCalledWith("/api/bank-connections/link-token", expect.objectContaining({
      method: "POST", headers: expect.objectContaining({ Authorization: "Bearer finance-session" }),
    }));
    expect(fetchMock.mock.calls[0][1].body).toBeUndefined();
  });

  it("sends Plaid's public token for exchange and returns the saved connection ID", async () => {
    const connection = { connection_id: "connection-test", provider: "plaid", status: "active" };
    respond(connection, 201);

    expect(await api.exchangeBankToken("public-test")).toEqual(connection);
    expect(fetchMock).toHaveBeenCalledWith("/api/bank-connections/exchange-token", expect.objectContaining({
      method: "POST", body: JSON.stringify({ public_token: "public-test" }),
      headers: expect.objectContaining({ Authorization: "Bearer finance-session" }),
    }));
    // neither Plaid's temporary token nor its result belongs in persistent browser storage
    expect(localStorage.length).toBe(1);
    expect(localStorage.getItem("access_token")).toBe("finance-session");
  });

  it("imports using the saved connection ID and returns the number of newly saved transactions", async () => {
    respond({ imported: 12 });

    expect(await api.importBankTransactions("connection-test")).toEqual({ imported: 12 });
    expect(fetchMock).toHaveBeenCalledWith("/api/bank-connections/connection-test/import", expect.objectContaining({
      method: "POST", headers: expect.objectContaining({ Authorization: "Bearer finance-session" }),
    }));
    expect(fetchMock.mock.calls[0][1].body).toBeUndefined();
  });

  it("passes a failed import back to the UI without reporting a successful count", async () => {
    fetchMock.mockResolvedValue({ status: 502, ok: false, text: async () => "Could not fetch bank transactions" });
    await expect(api.importBankTransactions("connection-test")).rejects.toThrow("Could not fetch bank transactions");
  });

  it("uses the existing logout behavior when a bank request returns 401", async () => {
    const logout = vi.fn();
    api.setUnauthorizedCallback(logout);
    respond("expired", 401);

    await expect(api.createBankLinkToken()).rejects.toThrow("Session expired. Please log in again.");
    expect(localStorage.getItem("access_token")).toBeNull();
    expect(logout).toHaveBeenCalledOnce();
  });
});
