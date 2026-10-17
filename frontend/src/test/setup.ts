import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

// Node's native storage can shadow jsdom's Storage implementation on newer runtimes.
const browserWindow = (globalThis as typeof globalThis & { jsdom: { window: Window } }).jsdom.window;
Object.defineProperty(globalThis, "localStorage", {
  value: browserWindow.localStorage, configurable: true, writable: true,
});
Object.defineProperty(globalThis, "sessionStorage", {
  value: browserWindow.sessionStorage, configurable: true, writable: true,
});

afterEach(() => {
  cleanup();
  localStorage.clear();
});
