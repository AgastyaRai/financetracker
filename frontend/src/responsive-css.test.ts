/// <reference types="node" />

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const appCss = readFileSync("src/App.css", "utf8");

describe("responsive styles", () => {
  it("scales the primary heading at the phone breakpoint", () => {
    expect(appCss).toMatch(
      /@media \(max-width: 600px\)\{\s+h1\{ font-size: clamp\(34px, 11vw, 44px\); \}/,
    );
  });

  it("provides phone-sized touch targets without enlarging checkboxes", () => {
    expect(appCss).toContain("button, input, select{ min-height: 44px; }");
    expect(appCss).toContain('input[type="checkbox"]{ min-height: 20px; min-width: 20px; }');
  });
});
