// This checks palette ownership, not contrast ratios. Contrast needs the actual
// Console theme in a browser; a second palette here would test the wrong colors.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const source = path.join(__dirname, '..', '..', 'dashboard', 'src');
const shell = fs.readFileSync(path.join(source, 'components/dashboard-shell.css'), 'utf8');

test('semantic colors resolve to host palette tokens', () => {
  for (const [local, host] of [
    ['bg', 'bg'], ['surface', 'panel'], ['surface-raised', 'panel-raised'],
    ['text', 'ink'], ['accent', 'accent'],
  ]) {
    assert.match(shell, new RegExp(`--${local}:\\s*var\\(--color-${host}\\)`));
  }
  assert.match(shell, /--text-soft:\s*color-mix\([^;]+var\(--color-ink-faint\)/);
  assert.match(shell, /--text-muted:\s*var\(--text-soft\)/);
  assert.doesNotMatch(shell, /data-mode="standalone"|data-theme="dark"|#[0-9a-f]{3,8}\b/i);
});

test('extension styles do not shadow shared host palette names', () => {
  const hostNames = ['bg', 'panel', 'panel-raised', 'surface', 'surface-hover', 'surface-selected', 'ink', 'ink-faint', 'ink-ghost', 'edge', 'accent', 'accent-muted', 'warn', 'alert', 'ok'];
  for (const file of fs.readdirSync(source, {recursive: true}).filter(file => file.endsWith('.css'))) {
    const css = fs.readFileSync(path.join(source, file), 'utf8').replace(/@theme reference\s*\{[^}]*\}/g, '');
    assert.doesNotMatch(css, new RegExp(`--color-(?:${hostNames.join('|')})\\s*:`), file);
  }
});
