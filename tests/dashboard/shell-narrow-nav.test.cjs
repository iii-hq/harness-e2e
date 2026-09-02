// Audit S-01 / RD-01: below 720px of container width the section navigation
// must switch from tabs to a select. Today the select is hidden by a Tailwind
// `hidden` utility (imported with `important`), which the CSS override in
// dashboard-shell.css cannot beat, so the navigation disappears. These checks
// describe the CSS-only toggle PR2 introduces; the `todo` ones pass once it
// lands and must then lose their `todo` option.
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

test(
  "the narrow select is hidden by CSS, not by a Tailwind utility",
  { todo: "S-01: DashboardShell.tsx still uses the `hidden` utility until PR2" },
  () => {
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
  },
);
