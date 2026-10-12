/// <reference types="node" />

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const indexHtml = readFileSync("index.html", "utf8");

describe("application shell branding", () => {
  it("uses FinanceTracker branding instead of Vite defaults", () => {
    expect(indexHtml).toContain("<title>FinanceTracker</title>");
    expect(indexHtml).toContain('href="/finance-tracker.svg"');
    expect(indexHtml).not.toContain("vite.svg");
  });
});
