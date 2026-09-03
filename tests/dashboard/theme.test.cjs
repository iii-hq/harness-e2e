const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const repositoryRoot = path.join(__dirname, "..", "..");
const dashboardRoot = path.join(repositoryRoot, "dashboard");

test("sets the stored or system theme before React renders", () => {
  const html = fs.readFileSync(path.join(dashboardRoot, "index.html"), "utf8");
  assert.match(html, /localStorage\.getItem\('harness-e2e-theme'\)/);
  assert.match(html, /prefers-color-scheme: dark/);
  assert.match(html, /document\.documentElement\.dataset\.theme = theme/);
  assert.doesNotMatch(html, /data-page=/);
});

test("uses the shared React theme control on every dashboard page", () => {
  const dashboardShell = fs.readFileSync(
    path.join(dashboardRoot, "src", "components", "DashboardShell.tsx"),
    "utf8",
  );
  assert.match(dashboardShell, /import \{ ThemeToggle \}/);
  assert.match(dashboardShell, /<ThemeToggle/);

  for (const page of [
    "OverviewPage.tsx",
    "TestsPage.tsx",
    "ExecutionPage.tsx",
    "CoveragePage.tsx",
    "TestsCatalogPage.tsx",
    "TestHistoryPage.tsx",
    "PlansPage.tsx",
    "LocalPlanPage.tsx",
  ]) {
    const source = fs.readFileSync(
      path.join(dashboardRoot, "src", "pages", page),
      "utf8",
    );
    assert.match(source, /import\s*\{[\s\S]*?DashboardPageActions/);
    assert.match(source, /<DashboardPageActions/);
  }
});

test("defines complete light and dark token sets", () => {
  // Audit DS-03: the palette lives in dashboard-shell.css and reaches the
  // standalone document root through data-harness-e2e, so legacy.css paints
  // nothing private. Both themes set their own color-scheme there.
  const shell = fs.readFileSync(
    path.join(dashboardRoot, "src", "components", "dashboard-shell.css"),
    "utf8",
  );
  const styles = fs.readFileSync(
    path.join(dashboardRoot, "src", "legacy.css"),
    "utf8",
  );

  assert.match(
    shell,
    /:root\[data-harness-e2e="standalone"\]\s*\{[^}]*color-scheme:\s*light;/s,
  );
  assert.match(
    shell,
    /:root\[data-harness-e2e="standalone"\]\[data-theme="dark"\]\s*\{[^}]*color-scheme:\s*dark;/s,
  );
  for (const token of ["--bg", "--surface", "--text", "--accent", "--info", "--danger"]) {
    assert.match(shell, new RegExp(`${token}:`));
  }
  assert.doesNotMatch(styles, /:root\s*\{[^}]*--(?:bg|surface|text|accent|danger):/s);
  assert.doesNotMatch(styles, /rgba\(\d/);
  assert.match(
    fs.readFileSync(path.join(dashboardRoot, "index.html"), "utf8"),
    /<html lang="en" data-harness-e2e="standalone">/,
  );
});
