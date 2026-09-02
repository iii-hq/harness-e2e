// Guard-rail for the shell's text tokens. Every token the dashboard uses for
// text below 18px must reach WCAG AA (4.5:1) on the panel, the raised panel
// and the .055 fill, in both themes; the control edge must reach 3:1 on the
// panel (WCAG 1.4.11). Tokens are read from dashboard-shell.css in standalone
// mode and resolved through their var() chains, so a change anywhere in the
// chain is measured. Audit ids: A11Y-01, A11Y-02, A11Y-09, DK-01, DK-03.
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
  const body = css.slice(open + 1, close).replace(/\/\*[\s\S]*?\*\//g, "");
  for (const match of body.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) {
    tokens[match[1]] = match[2].replace(/\s+/g, " ").trim();
  }
  return tokens;
}

// Cascade order for a token: shell block, then (dark) shell dark block, then
// the standalone block(s), which come later in the file at equal specificity.
const blocks = {
  shell: tokenBlock(/\.harness-e2e-shell\.harness-e2e-shell,/),
  shellDark: tokenBlock(/\.harness-e2e-shell\.harness-e2e-shell\[data-theme="dark"\],/),
  standalone: tokenBlock(/\.harness-e2e-shell\[data-mode="standalone"\]\s*\{/),
  standaloneDark: tokenBlock(/\.harness-e2e-shell\[data-mode="standalone"\]\[data-theme="dark"\]\s*\{/),
};
const scopes = {
  light: { ...blocks.shell, ...blocks.standalone },
  dark: { ...blocks.shell, ...blocks.shellDark, ...blocks.standalone, ...blocks.standaloneDark },
};

// Resolves var() chains, rgb(<channel list>) and colour literals to [r,g,b,a].
function resolve(value, scope, depth = 0) {
  assert.ok(depth < 12, `token chain too deep: ${value}`);
  let text = value;
  for (let guard = 0; guard < 12 && /var\(/.test(text); guard += 1) {
    text = text.replace(/var\((--[\w-]+)\s*(?:,\s*((?:[^()]|\([^()]*\))*))?\)/g, (_all, name, fallback) => {
      if (scope[name] !== undefined) return scope[name];
      assert.ok(fallback !== undefined, `${name} is undefined in this scope and has no fallback`);
      return fallback;
    });
  }
  return parseColor(text.trim(), scope, depth);
}

function parseColor(text, scope, depth) {
  const hex = text.match(/^#([0-9a-f]{6})$/i);
  if (hex) {
    const n = Number.parseInt(hex[1], 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255, 1];
  }
  const rgb = text.match(/^rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*([\d.]+))?\s*\)$/);
  if (rgb) return [Number(rgb[1]), Number(rgb[2]), Number(rgb[3]), rgb[4] === undefined ? 1 : Number(rgb[4])];
  const mix = text.match(/^color-mix\(in srgb,\s*(.+?)\s+(\d+)%,\s*transparent\)$/);
  if (mix) {
    const inner = resolve(mix[1], scope, depth + 1);
    return [inner[0], inner[1], inner[2], (inner[3] * Number(mix[2])) / 100];
  }
  assert.fail(`unsupported colour: ${text}`);
}

function over([r, g, b, a], [br, bg, bb]) {
  return [Math.round(r * a + br * (1 - a)), Math.round(g * a + bg * (1 - a)), Math.round(b * a + bb * (1 - a))];
}

function luminance([r, g, b]) {
  const channel = (value) => {
    const c = value / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

function contrast(foreground, background) {
  const fg = luminance(over(foreground, background));
  const bg = luminance(background);
  const [hi, lo] = fg > bg ? [fg, bg] : [bg, fg];
  return Math.round(((hi + 0.05) / (lo + 0.05)) * 100) / 100;
}

function grounds(scope) {
  const panel = over(resolve("var(--surface)", scope), [0, 0, 0]);
  return {
    panel,
    raised: over(resolve("var(--surface-raised)", scope), [0, 0, 0]),
    fill: over(resolve("var(--surface-fill)", scope), panel),
  };
}

// Every token the dashboard paints small text with.
const textTokens = ["--text", "--text-soft", "--text-muted", "--accent", "--success", "--warning", "--danger"];
const AA = 4.5;

for (const theme of ["light", "dark"]) {
  test(`${theme}: text tokens reach ${AA}:1 on panel, raised panel and fill`, () => {
    const scope = scopes[theme];
    const surfaces = grounds(scope);
    const rows = [];
    const failures = [];
    for (const token of textTokens) {
      const colour = resolve(`var(${token})`, scope);
      for (const [ground, rgb] of Object.entries(surfaces)) {
        const ratio = contrast(colour, rgb);
        rows.push({ theme, token, ground, ratio });
        if (ratio < AA) failures.push(`${theme} ${token} on ${ground}: ${ratio}:1`);
      }
    }
    console.table(rows);
    assert.deepEqual(failures, [], `text tokens below ${AA}:1`);
  });

  test(`${theme}: the control edge reaches 3:1 on the panel`, () => {
    const scope = scopes[theme];
    const { panel } = grounds(scope);
    const ratio = contrast(resolve("var(--control-edge)", scope), panel);
    assert.ok(ratio >= 3, `${theme} --control-edge on panel is ${ratio}:1`);
  });
}

test("decorative ink stays separate from text ink", () => {
  for (const theme of ["light", "dark"]) {
    const scope = scopes[theme];
    assert.ok(scope["--ink-decor"], `${theme} declares --ink-decor`);
    assert.notEqual(
      resolve("var(--ink-decor)", scope).join(","),
      resolve("var(--text-muted)", scope).join(","),
      `${theme}: --ink-decor and --text-muted must differ; decoration is not text`,
    );
  }
});

test("legacy tokens the old stylesheet still reads are defined", () => {
  for (const token of ["--font-body", "--contrast", "--code-bg", "--code-text"]) {
    assert.ok(blocks.shell[token], `${token} is defined in the shell block (audit PD-02 / DS-03)`);
  }
});
