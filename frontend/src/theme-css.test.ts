/// <reference types="node" />

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const appCss = readFileSync("src/App.css", "utf8");
const indexCss = readFileSync("src/index.css", "utf8");

describe("dark theme boundaries", () => {
  it("uses distinct outlines for major sections and internal boxed surfaces", () => {
    expect(indexCss).toContain("--border: #1d2438;");
    expect(indexCss).toContain("--border-strong: #2f3b73;");
    expect(indexCss).toContain("--section-border: #111216;");
    expect(appCss.match(/border: 1px solid var\(--border\);/g)?.length ?? 0).toBeGreaterThanOrEqual(5);
    expect(appCss.match(/border: 3px solid var\(--border-strong\);/g)?.length ?? 0).toBeGreaterThanOrEqual(2);
    expect(appCss).toContain("border: 3px solid transparent;");
  });
});
