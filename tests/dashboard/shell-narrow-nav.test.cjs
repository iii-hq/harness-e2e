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

test("page actions have a narrow-container disclosure (S-02)", () => {
  assert.match(shellTsx, /harness-e2e-header-overflow/);
  assert.match(shellCss, /\.harness-e2e-header-overflow-menu\s*\{[^}]*position:\s*absolute/);
});
