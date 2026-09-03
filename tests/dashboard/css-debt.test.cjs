// Ratchet for the dashboard's visual-vocabulary debt. Every metric counts a
// pattern the design system forbids (1px borders as hierarchy, radii other
// than the 6px token, text below 11px, shadows, !important, arbitrary
// Tailwind sizes). The baseline can only go down: a pull request that adds
// debt fails here, and a migration that removes debt lowers the baseline with
//
//   CSS_DEBT_UPDATE=1 node --test tests/dashboard/css-debt.test.cjs
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const dashboardSrc = path.join(__dirname, "..", "..", "dashboard", "src");
const baselinePath = path.join(__dirname, "css-debt.baseline.json");

function walk(dir, out = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(full, out);
    else out.push(full);
  }
  return out;
}

const files = walk(dashboardSrc).filter((file) => !/\.test\.[cm]?[jt]sx?$/.test(file));
const cssFiles = files.filter((file) => file.endsWith(".css"));
// The design system is the one place a 1px line is sanctioned (table row
// dividers, see primitives.css); every other stylesheet must keep shedding them.
const legacyCssFiles = cssFiles.filter((file) => !file.includes(`${path.sep}design-system${path.sep}`));
const tsxFiles = files.filter((file) => /\.(tsx|ts)$/.test(file));
const read = (file) => fs.readFileSync(file, "utf8");
const countIn = (list, regex) =>
  list.reduce((total, file) => total + (read(file).match(regex) ?? []).length, 0);

function toPx(value, unit) {
  if (unit === "px") return value;
  return value * 16; // rem/em: the dashboard root is 16px
}

function cssFontSizesBelow11() {
  let total = 0;
  for (const file of cssFiles) {
    for (const match of read(file).matchAll(/font-size\s*:\s*(\d*\.?\d+)(px|rem|em)\b/g)) {
      if (toPx(Number(match[1]), match[2]) < 11) total += 1;
    }
  }
  return total;
}

function tsxArbitraryTextBelow11() {
  let total = 0;
  for (const file of tsxFiles) {
    for (const match of read(file).matchAll(/text-\[(\d*\.?\d+)(px|rem|em)\]/g)) {
      if (toPx(Number(match[1]), match[2]) < 11) total += 1;
    }
  }
  return total;
}

function cssRadiiOffToken() {
  let total = 0;
  for (const file of cssFiles) {
    for (const match of read(file).matchAll(/border-radius\s*:\s*([^;}]+)/g)) {
      const value = match[1].trim();
      if (/^(6px|0|0px|50%|inherit|var\()/.test(value)) continue;
      total += 1;
    }
  }
  return total;
}

const metrics = {
  // rgba/hex borders drawn as 1px lines: the DS uses fills for hierarchy
  cssBorder1px: () =>
    countIn(legacyCssFiles, /border(?:-(?:top|right|bottom|left|inline|block)(?:-start|-end)?)?\s*:\s*1px/g),
  cssRadiiOffToken,
  cssFontSizesBelow11,
  cssBoxShadow: () => countIn(cssFiles, /box-shadow\s*:\s*(?!none\b)[^;}]+/g),
  cssImportant: () => countIn(cssFiles, /!important/g),
  cssLinesLegacy: () => read(path.join(dashboardSrc, "legacy.css")).split("\n").length,
  tsxArbitraryTextBelow11,
  tsxRoundedOffToken: () => countIn(tsxFiles, /\brounded-(?:full|sm|md|lg|xl|2xl|3xl|\[(?!6px\])[^\]]+\])/g),
  tsxBorderUtility: () => countIn(tsxFiles, /(?<=["'\s])border(?:-[trblxyse])?(?=["'\s])/g),
};

const current = Object.fromEntries(Object.entries(metrics).map(([name, count]) => [name, count()]));

if (process.env.CSS_DEBT_UPDATE) {
  const previous = fs.existsSync(baselinePath) ? JSON.parse(fs.readFileSync(baselinePath, "utf8")) : {};
  const next = {};
  for (const [name, value] of Object.entries(current)) {
    next[name] = process.env.CSS_DEBT_UPDATE === "force" ? value : Math.min(value, previous[name] ?? value);
  }
  fs.writeFileSync(baselinePath, `${JSON.stringify(next, null, 2)}\n`);
  console.log("css-debt baseline written", next);
}

const baseline = JSON.parse(fs.readFileSync(baselinePath, "utf8"));

test("dashboard css debt never grows (ratchet)", () => {
  const rows = Object.entries(current).map(([name, value]) => ({
    metric: name,
    baseline: baseline[name],
    current: value,
    delta: value - (baseline[name] ?? value),
  }));
  console.table(rows);
  const grown = rows.filter((row) => row.baseline !== undefined && row.current > row.baseline);
  assert.deepEqual(
    grown.map((row) => `${row.metric}: ${row.current} > ${row.baseline}`),
    [],
    "new visual debt was added; use the design-system primitives or lower the count elsewhere",
  );
  for (const name of Object.keys(metrics)) {
    assert.ok(name in baseline, `missing baseline for ${name}; run with CSS_DEBT_UPDATE=1`);
  }
});

test("a migration that removes debt also lowers the baseline", () => {
  const stale = Object.entries(current).filter(
    ([name, value]) => baseline[name] !== undefined && value < baseline[name] - 5,
  );
  assert.deepEqual(
    stale.map(([name, value]) => `${name}: ${value} < baseline ${baseline[name]}`),
    [],
    "debt went down; lock it in with CSS_DEBT_UPDATE=1 so it cannot creep back",
  );
});
