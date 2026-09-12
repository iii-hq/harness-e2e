const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const root = path.join(__dirname, '..', '..', 'dashboard');
const read = file => fs.readFileSync(path.join(root, file), 'utf8');

test('the extension receives its theme from the Console host', () => {
  assert.match(read('src/console-entry.tsx'), /host\.useTheme\(\)/);
  assert.doesNotMatch(read('src/components/DashboardShell.tsx'), /ThemeToggle|matchMedia|localStorage/);
  assert.equal(fs.existsSync(path.join(root, 'src/components/ThemeToggle.tsx')), false);
  assert.equal(fs.existsSync(path.join(root, 'index.html')), false);
});

test('extension typography inherits host fonts without bundling font faces', () => {
  const shell = read('src/components/dashboard-shell.css');
  const entry = read('src/index.css');
  assert.match(shell, /font-family:\s*var\(--font-sans\)/);
  assert.doesNotMatch(shell + entry.replace(/@theme reference\s*\{[^}]*\}/g, ''), /@font-face|@fontsource|--font-(?:sans|mono)\s*:/);
});
