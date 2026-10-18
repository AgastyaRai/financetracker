const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { before, after, test } = require("node:test");
const { chromium } = require("playwright");

// Run against a production build with Playwright available on NODE_PATH.
// FRONTEND_DIST selects the build; PLAYWRIGHT_CHANNEL can select an installed browser.
assert.ok(process.env.FRONTEND_DIST, "Set FRONTEND_DIST to the frontend production build directory");
const dist = path.resolve(process.env.FRONTEND_DIST);
const artifacts = process.env.BROWSER_ARTIFACT_DIR;
let browser;

before(async () => {
  browser = await chromium.launch({ headless: true, channel: process.env.PLAYWRIGHT_CHANNEL });
});
after(async () => { await browser?.close(); });

for (const [size, width] of [["desktop", 1280], ["mobile", 390]]) {
  test(`conflict recovery controls fit and reload/save works on ${size}`, async () => {
    const page = await browser.newPage({ viewport: { width, height: 1000 } });
    const month = new Date().toISOString().slice(0, 7);
    let saved = {
      id: "layout-coffee", amount: "-10.00", kind: "Expense", category: "Food",
      date: month + "-01", description: "Imported coffee", row_version: 1, can_revert: false,
    };
    const updates = [];
    try {
      await page.route("**/*", async (route) => {
        const request = route.request();
        const url = new URL(request.url());
        if (url.hostname !== "row-version-browser.invalid") return route.abort();
        const json = (value, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(value) });
        if (url.pathname === "/api/transactions" && request.method() === "GET") return json([saved]);
        if (url.pathname === "/api/transactions/layout-coffee" && request.method() === "PUT") {
          const input = request.postDataJSON();
          updates.push(input);
          if (input.expected_version !== saved.row_version) return json({ error: "Transaction changed. Reload latest before saving." }, 409);
          saved = { ...saved, ...input, row_version: saved.row_version + 1 };
          return route.fulfill({ status: 200, body: "" });
        }
        if (url.pathname === "/api/transactions/indexing-status") return json({ total: 1, indexed: 1, remaining: 0 });
        if (url.pathname === "/api/bank-connections" || url.pathname.startsWith("/api/budgets")) return json([]);
        assert.ok(!url.pathname.startsWith("/api/"), `Unexpected mocked request: ${url.pathname}`);
        const filename = path.join(dist, url.pathname === "/" ? "index.html" : url.pathname);
        const contentTypes = { ".html": "text/html", ".js": "application/javascript", ".css": "text/css", ".woff2": "font/woff2" };
        return route.fulfill({ contentType: contentTypes[path.extname(filename)] || "application/octet-stream", body: fs.readFileSync(filename) });
      });
      await page.addInitScript(() => {
        const payload = btoa(JSON.stringify({ exp: Math.floor(Date.now() / 1000) + 3600 }));
        localStorage.setItem("access_token", `header.${payload}.signature`);
      });
      await page.goto("http://row-version-browser.invalid/");
      await page.getByRole("button", { name: /Edit transaction: Imported coffee/ }).click();
      const editor = page.locator("tr.transactionEditingRow");
      await editor.getByLabel("Description (optional)", { exact: true }).fill("Unsaved note");
      saved = { ...saved, amount: "-12.00", date: month + "-02", row_version: 2 };
      await page.getByRole("button", { name: "Save changes", exact: true }).click();
      await page.getByRole("alert").filter({ hasText: "Transaction changed." }).waitFor();
      assert.equal(await editor.getByLabel("Description (optional)", { exact: true }).inputValue(), "Unsaved note");

      const reload = page.getByRole("button", { name: "Reload latest", exact: true });
      await reload.scrollIntoViewIfNeeded();
      const geometry = await reload.evaluate((element) => {
        const bounds = element.getBoundingClientRect();
        let left = 0, right = window.innerWidth;
        for (let parent = element.parentElement; parent; parent = parent.parentElement) {
          if (["hidden", "clip", "auto", "scroll"].includes(getComputedStyle(parent).overflowX)) {
            const rect = parent.getBoundingClientRect();
            left = Math.max(left, rect.left);
            right = Math.min(right, rect.right);
          }
        }
        return { height: bounds.height, left: bounds.left, right: bounds.right, visibleLeft: left, visibleRight: right };
      });
      if (artifacts) {
        fs.mkdirSync(artifacts, { recursive: true });
        const section = page.locator(".card").filter({ has: page.getByRole("heading", { name: "Transactions", exact: true }) });
        await section.screenshot({ path: path.join(artifacts, `reload-${size}.png`) });
        fs.writeFileSync(path.join(artifacts, `reload-${size}.json`), JSON.stringify(geometry, null, 2));
      }
      assert.ok(geometry.height <= 64, `Reload label should fit a normal control, observed ${JSON.stringify(geometry)}`);
      assert.ok(geometry.left >= geometry.visibleLeft - 1 && geometry.right <= geometry.visibleRight + 1, `Reload must not be clipped: ${JSON.stringify(geometry)}`);

      await reload.click();
      await page.waitForFunction(() => document.querySelector('tr.transactionEditingRow input[aria-label="Amount"]')?.value === "-12.00");
      assert.equal(await editor.getByLabel("Date", { exact: true }).inputValue(), month + "-02");
      assert.equal(await editor.getByLabel("Description (optional)", { exact: true }).inputValue(), "Imported coffee");
      assert.equal(updates.length, 1, "Reload must not automatically save");
      await editor.getByLabel("Description (optional)", { exact: true }).fill("Reviewed note");
      await page.getByRole("button", { name: "Save changes", exact: true }).click();
      await page.getByText("Transaction updated successfully.", { exact: true }).waitFor();
      assert.equal(updates.length, 2);
      assert.equal(updates[1].expected_version, 2);
      assert.equal(updates[1].amount, "-12.00");
      assert.equal(saved.description, "Reviewed note");
    } finally { await page.close(); }
  });
}
