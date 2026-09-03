// Guard-rail for the shell's colour tokens. Every token that carries text must
// reach WCAG AA (4.5:1) on the panel, the raised panel and the fill it sits on,
// in both themes. Tokens that fail today (audit A11Y-01, A11Y-02, DK-01) carry
// a lower floor equal to their current ratio, so they cannot get worse; PR1 of
// the UI roadmap raises the tokens and must raise these floors to 4.5.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const css = fs.readFileSync(
  path.join(__dirname, "..", "..", "dashboard", "src", "components", "dashboard-shell.css"),
  "utf8",
);

function tokenBlock(selector) {
  const start = css.search(selector);
  assert.ok(start >= 0, `selector not found: ${selector}`);
  const open = css.indexOf("{", start);
  const close = css.indexOf("}", open);
  const tokens = {};
  for (const match of css.slice(open + 1, close).matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) {
    tokens[match[1]] = match[2].trim();
  }
  return tokens;
}

function parseColor(value) {
  const hex = value.match(/^#([0-9a-f]{6})$/i);
  if (hex) {
    const n = Number.parseInt(hex[1], 16);
    return { rgb: [(n >> 16) & 255, (n >> 8) & 255, n & 255], alpha: 1 };
  }
  const rgba = value.match(/^rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*([\d.]+))?\s*\)$/);
  assert.ok(rgba, `unsupported colour: ${value}`);
  return {
    rgb: [Number(rgba[1]), Number(rgba[2]), Number(rgba[3])],
    alpha: rgba[4] === undefined ? 1 : Number(rgba[4]),
  };
}

function composite(foreground, background) {
  const { rgb, alpha } = parseColor(foreground);
  return rgb.map((channel, index) => Math.round(channel * alpha + background[index] * (1 - alpha)));
}

function luminance([r, g, b]) {
  const channel = (value) => {
    const c = value / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

function contrastRatio(text, surface) {
  const fg = luminance(composite(text, surface));
  const bg = luminance(surface);
  const [hi, lo] = fg > bg ? [fg, bg] : [bg, fg];
  return Math.round(((hi + 0.05) / (lo + 0.05)) * 100) / 100;
}

const themes = {
  light: tokenBlock(/\.harness-e2e-shell\[data-mode="standalone"\]\s*\{/),
  dark: tokenBlock(/\.harness-e2e-shell\[data-mode="standalone"\]\[data-theme="dark"\]\s*\{/),
};

function surfaces(theme) {
  const tokens = themes[theme];
  const panel = parseColor(tokens["--color-panel"]).rgb;
  return {
    panel,
    raised: parseColor(tokens["--color-panel-raised"]).rgb,
    fill: composite(tokens["--color-surface"], panel),
  };
}

const AA = 4.5;
// Floors below AA are the ratios measured in the audit; they only go up.
const floors = {
  light: {
    "--color-ink": AA,
    "--color-ink-faint": AA,
    "--color-ink-ghost": 2.2, // A11Y-01: 2.49 on panel, 2.22 on fill
    "--color-accent": AA,
    "--color-ok": AA,
    "--color-warn": 3.2, // A11Y-02: 3.69 on panel, 3.29 on fill
    "--color-alert": 3.4, // A11Y-02: 3.81 on panel, 3.40 on fill
  },
  dark: {
    "--color-ink": AA,
    "--color-ink-faint": AA,
    "--color-ink-ghost": 3.3, // DK-01: 3.76 on panel, 3.32 on fill
    "--color-accent": AA,
    "--color-ok": AA,
    "--color-warn": AA,
    "--color-alert": AA,
  },
};

for (const theme of Object.keys(floors)) {
  test(`${theme} text tokens keep their contrast floor (target ${AA}:1)`, () => {
    const tokens = themes[theme];
    const grounds = surfaces(theme);
    const rows = [];
    const failures = [];
    for (const [token, floor] of Object.entries(floors[theme])) {
      assert.ok(tokens[token], `${token} is declared for ${theme}`);
      for (const [ground, rgb] of Object.entries(grounds)) {
        const ratio = contrastRatio(tokens[token], rgb);
        rows.push({ theme, token, ground, ratio, floor, aa: ratio >= AA ? "yes" : "no" });
        if (ratio < floor) failures.push(`${theme} ${token} on ${ground}: ${ratio}:1 < floor ${floor}:1`);
      }
    }
    console.table(rows);
    assert.deepEqual(failures, [], "a text token lost contrast");
  });
}

test("tokens that already reach AA are pinned at 4.5 (raise the floor when fixing the rest)", () => {
  for (const theme of Object.keys(floors)) {
    const tokens = themes[theme];
    const grounds = surfaces(theme);
    for (const [token, floor] of Object.entries(floors[theme])) {
      if (floor >= AA) continue;
      const reachesEverywhere = Object.values(grounds).every((rgb) => contrastRatio(tokens[token], rgb) >= AA);
      assert.equal(
        reachesEverywhere,
        false,
        `${theme} ${token} now reaches ${AA}:1 everywhere; raise its floor to ${AA} in this test`,
      );
    }
  }
});

test("control edges: the edge token against the panel (WCAG 1.4.11 wants 3:1)", { todo: "DK-03 / A11Y-09: fill-only fields until PR1 adds a 3:1 edge" }, () => {
  for (const theme of Object.keys(floors)) {
    const ratio = contrastRatio(themes[theme]["--color-edge"], surfaces(theme).panel);
    assert.ok(ratio >= 3, `${theme} --color-edge on panel is ${ratio}:1`);
  }
});
