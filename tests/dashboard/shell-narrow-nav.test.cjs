// Audit S-01 / RD-01: below 720px of container width the section navigation
// switches from tabs to a select. Both states are decided by
// dashboard-shell.css keyed on data-narrow; no Tailwind utility (imported
// with `important`) may take part, or the navigation disappears again.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const componentsDir = path.join(__dirname, "..", "..", "dashboard", "src", "components");
const shellCss = fs.readFileSync(path.join(componentsDir, "dashboard-shell.css"), "utf8");
const shellTsx = fs.readFileSync(path.join(componentsDir, "DashboardShell.tsx"), "utf8");

test("the shell exposes a narrow select and a wide tab list", () => {
  assert.match(shellTsx, /harness-e2e-navigation-narrow/);
  assert.match(shellTsx, /harness-e2e-navigation-wide/);
  assert.match(shellCss, /\[data-narrow="true"\]\s+\.harness-e2e-navigation-narrow\s*\{[^}]*display:\s*block/);
  assert.match(shellCss, /\[data-narrow="true"\]\s+\.harness-e2e-navigation-wide\s*\{[^}]*display:\s*none/);
});

test("the narrow select is hidden by CSS, not by a Tailwind utility", () => {
  assert.equal(
    /harness-e2e-navigation-narrow[^"]*\bhidden\b/.test(shellTsx),
    false,
    "DashboardShell.tsx hides the narrow select with the Tailwind `hidden` utility",
  );
  assert.equal(
    /\.harness-e2e-navigation-narrow\s*\{[^}]*display:\s*none/.test(shellCss),
    true,
    "dashboard-shell.css does not hide the narrow select by default",
  );
});

// Audit S-02 / S-05: page actions no longer sit in the console header where
// they pushed the close control out of view; they share the section bar
// inside the page and wrap in narrow containers.
test("page actions live in the wrapping section bar, not the console header (S-02)", () => {
  assert.match(shellTsx, /<PageActionsBar/);
  assert.match(shellTsx, /className="harness-e2e-page-actions"/);
  assert.match(shellCss, /\.harness-e2e-page-actions\s*\{[^}]*flex-wrap:\s*wrap/);
  assert.doesNotMatch(shellTsx, /harness-e2e-header-overflow/);
  assert.doesNotMatch(shellCss, /harness-e2e-header-overflow/);
});

// Redesign canvas: the section bar is a raised strip of sentence-case Inter
// tabs with an ink underline on the current one; the actions are Inter too.
test("tabs and page actions are sentence-case Inter, not mono lowercase", () => {
  const rule = (selector) =>
    shellCss.match(new RegExp(`\\n\\s*${selector.replace(/[.[\]"=]/g, "\\$&")}\\s*\\{([^}]*)\\}`))?.[1] ?? "";
  for (const selector of [".harness-e2e-nav-link", ".harness-e2e-nav-select", ".harness-e2e-header-action"]) {
    const body = rule(selector);
    assert.match(body, /font-family:\s*var\(--font-sans\)/, selector);
    assert.doesNotMatch(body, /text-transform:\s*lowercase/, selector);
  }
  assert.match(rule(".harness-e2e-navigation"), /background:\s*var\(--color-panel-raised\)/);
  assert.match(rule('.harness-e2e-nav-link[aria-current="page"]::after'), /height:\s*2px[^}]*background:\s*var\(--color-ink\)/);
  const actionsTsx = fs.readFileSync(path.join(componentsDir, "DashboardPageActions.tsx"), "utf8");
  assert.doesNotMatch(actionsTsx, /font-mono|lowercase/);
});
