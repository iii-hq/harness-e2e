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
  for (const page of [
    "OverviewPage.tsx",
    "TestsPage.tsx",
    "ExecutionPage.tsx",
    "CoveragePage.tsx",
  ]) {
    const source = fs.readFileSync(
      path.join(dashboardRoot, "src", "pages", page),
      "utf8",
    );
    assert.match(source, /import \{ ThemeToggle \}/);
    assert.match(source, /<ThemeToggle \/>/);
  }
});

test("defines complete light and dark token sets", () => {
  const styles = fs.readFileSync(
    path.join(dashboardRoot, "src", "index.css"),
    "utf8",
  );

  assert.match(styles, /:root\s*\{[^}]*color-scheme:\s*dark;/s);
  assert.match(
    styles,
    /:root\[data-theme="light"\]\s*\{[^}]*color-scheme:\s*light;/s,
  );
  for (const token of ["--bg", "--surface", "--text", "--accent", "--danger"]) {
    assert.match(styles, new RegExp(`${token}:`));
  }
});
