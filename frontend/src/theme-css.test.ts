/// <reference types="node" />

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const appCss = readFileSync("src/App.css", "utf8");
const indexCss = readFileSync("src/index.css", "utf8");

describe("dark theme boundaries", () => {
  it("uses stronger three-pixel outlines for primary boxed surfaces", () => {
    expect(indexCss).toContain("--border: #1d2438;");
    expect(indexCss).toContain("--border-strong: #2f3b73;");
    expect(appCss.match(/border: 1px solid var\(--border\);/g)?.length ?? 0).toBeGreaterThanOrEqual(6);
    expect(appCss.match(/border: 3px solid var\(--border-strong\);/g)?.length ?? 0).toBeGreaterThanOrEqual(3);
  });
});
