import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getTransactions, setUnauthorizedCallback } from "./api";

describe("API requests", () => {
  const fetchMock = vi.fn();

  beforeEach(() => {
    fetchMock.mockReset();
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
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
});
