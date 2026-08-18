import { readFileSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig, type Plugin } from 'vite'

const root = fileURLToPath(new URL('.', import.meta.url))
const scope = '[data-iii-ui="harness-e2e"]'
const dashboardRoot = '[data-harness-e2e-dashboard]'

function emptyFontAssets(): Plugin {
  const emptyId = '\0harness-e2e-empty-font-css'
  return {
    name: 'harness-e2e-empty-font-assets',
    enforce: 'pre',
    transform(source, id) {
      if (!id.endsWith('.css')) return null
      return source.replace(
        /^@import\s+["']@fontsource\/[^"']+["'];?\s*$/gm,
        '',
      )
    },
    resolveId(source) {
      if (source.startsWith('@fontsource/')) return emptyId
      return null
    },
    load(id) {
      if (id === emptyId) return ''
      return null
    },
  }
}

function scopedConsoleStyles(): Plugin {
  return {
    name: 'harness-e2e-scope-console-styles',
    enforce: 'post',
    writeBundle(options, bundle) {
      for (const item of Object.values(bundle)) {
        if (item.type !== 'asset' || !item.fileName.endsWith('.css')) continue
        const outputDir = options.dir ?? path.dirname(options.file ?? '')
        const outputPath = path.resolve(root, outputDir, item.fileName)
        const css = readFileSync(outputPath, 'utf8')
        if (css.includes('@font-face')) {
          throw new Error('injectable console CSS must not contain @font-face')
        }
        const sanitized = sanitizeConsoleStyles(css)
        const scoped = scopeCss(renameKeyframes(sanitized))
        if (!scoped.includes(scope)) {
          throw new Error(
            `injectable console CSS is missing its ${scope} scope`,
          )
        }
        const privateDecoration = scoped.match(
          /(?:gradient\(|@font-face|(?:^|[;{])\s*box-shadow\s*:)/i,
        )
        if (privateDecoration) {
          throw new Error(
            `injectable console CSS contains a private decoration or font rule: ${scoped.slice(Math.max(0, privateDecoration.index ?? 0) - 80, (privateDecoration.index ?? 0) + 120)}`,
          )
        }
        writeFileSync(outputPath, scoped)
      }
    },
  }
}

/**
 * The host owns color, type, elevation, and background treatment. The
 * standalone stylesheet still contains historical dashboard decorations, so
 * remove those declarations at the Console boundary instead of allowing a
 * worker bundle to repaint the host. Literal colors are converted to semantic
 * variables so dark/light changes continue to come from the host tokens.
 */
function sanitizeConsoleStyles(css: string): string {
  css = stripPrivateDeclarations(css)
  css = css.replace(/#([0-9a-f]{3,8})\b/gi, (_match, value: string) =>
    semanticHex(value),
  )

  const channels: Record<string, string> = {
    '199,255,74': '--color-accent',
    '123,199,255': '--color-accent',
    '158,230,108': '--color-ok',
    '255,209,102': '--color-warn',
    '255,120,111': '--color-alert',
    '255,0,38': '--color-alert',
    '255,255,255': '--color-ink',
    '0,0,0': '--color-bg',
    '20,16,8': '--color-ink',
    '13,16,14': '--color-panel',
  }
  css = css.replace(
    /rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)(?:\s*,\s*([\d.]+))?\s*\)/gi,
    (_match, red: string, green: string, blue: string, alpha?: string) => {
      const token = channels[`${red},${green},${blue}`]
      if (!token) return _match
      if (alpha === undefined) return `var(${token})`
      const percentage = Math.round(Number(alpha) * 100)
      return `color-mix(in srgb, var(${token}) ${percentage}%, transparent)`
    },
  )
  return css
}

function stripPrivateDeclarations(css: string): string {
  const declaration =
    /(^|[;{])(\s*)(background(?:-image)?|(?:-webkit-)?mask-image|box-shadow)\s*:/gi
  let output = ''
  let cursor = 0
  let match: RegExpExecArray | null
  while (true) {
    match = declaration.exec(css)
    if (!match) break
    const propertyStart = match.index + match[1].length
    let index = declaration.lastIndex
    let parens = 0
    let end = css.length
    for (; index < css.length; index++) {
      const char = css[index]
      if (char === '(') parens++
      else if (char === ')') parens = Math.max(0, parens - 1)
      else if (parens === 0 && char === ';') {
        end = index + 1
        break
      } else if (parens === 0 && char === '}') {
        end = index
        break
      }
    }
    const value = css.slice(declaration.lastIndex, end)
    const privateValue =
      match[3].toLowerCase() === 'box-shadow' || /gradient\(/i.test(value)
    output += css.slice(cursor, privateValue ? propertyStart : end)
    cursor = end
    // Keep the delimiter in the next search so adjacent declarations after a
    // semicolon are not skipped.
    declaration.lastIndex = Math.max(cursor - 1, 0)
  }
  return output + css.slice(cursor)
}

function semanticHex(value: string): string {
  const normalized = value.toLowerCase()
  const base =
    normalized.length === 3 || normalized.length === 4
      ? normalized
          .slice(0, 3)
          .split('')
          .map((c) => c + c)
          .join('')
      : normalized.slice(0, 6)
  const alphaDigits =
    normalized.length === 4 || normalized.length === 8
      ? normalized.slice(-2)
      : null
  const tokens: Record<string, string> = {
    c7ff4a: '--color-accent',
    b8420f: '--color-accent',
    '7bc7ff': '--color-accent',
    '9ee66c': '--color-ok',
    ffd166: '--color-warn',
    ff786f: '--color-alert',
    ff0026: '--color-alert',
    f05d68: '--color-alert',
    f5a524: '--color-warn',
    '356f3d': '--color-ok',
    '0a0a0a': '--color-bg',
    '0a0c0b': '--color-bg',
    '080a09': '--color-bg',
    '111111': '--color-panel',
    '111412': '--color-panel',
    '171717': '--color-panel-raised',
    '171a18': '--color-panel-raised',
    '1d211e': '--color-surface-hover',
    fafafa: '--color-panel',
    f7f5f2: '--color-panel-raised',
    f5f7f4: '--color-ink',
    ededed: '--color-ink',
    a8b0a9: '--color-ink-faint',
    a6a6a6: '--color-ink-faint',
    '707971': '--color-ink-ghost',
    '6f6f6f': '--color-ink-ghost',
    cbd3cc: '--color-ink-faint',
  }
  const token = tokens[base] ?? '--color-ink'
  if (!alphaDigits) return `var(${token})`
  const percentage = Math.round((Number.parseInt(alphaDigits, 16) / 255) * 100)
  if (percentage === 0) return 'transparent'
  if (percentage === 100) return `var(${token})`
  return `color-mix(in srgb, var(${token}) ${percentage}%, transparent)`
}

function renameKeyframes(css: string): string {
  const names = new Map<string, string>()
  for (const match of css.matchAll(/@(?:-webkit-)?keyframes\s+([\w-]+)/g)) {
    const name = match[1]
    if (!name.startsWith('harness-e2e-')) {
      names.set(name, `harness-e2e-${name}`)
    }
  }
  for (const [name, replacement] of names) {
    const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
    css = css.replace(
      new RegExp(`(@(?:-webkit-)?keyframes\\s+)${escaped}(?=\\s*\\{)`, 'g'),
      `$1${replacement}`,
    )
    css = css.replace(
      /(\b(?:-webkit-)?animation(?:-name)?\s*:[^;}]+)/gi,
      (declaration) =>
        declaration.replace(
          new RegExp(`(?<![\\w-])${escaped}(?![\\w-])`, 'g'),
          replacement,
        ),
    )
  }
  return css
}

function scopeCss(css: string): string {
  return rewriteRuleList(css)
}

function rewriteRuleList(css: string): string {
  let output = ''
  let cursor = 0
  while (cursor < css.length) {
    const open = findNextBrace(css, cursor)
    if (open < 0) return output + css.slice(cursor)
    const close = findMatchingBrace(css, open)
    if (close < 0) throw new Error('unbalanced CSS in injectable bundle')

    const rawPrelude = css.slice(cursor, open)
    const boundary = lastTopLevelSemicolon(rawPrelude)
    const leading = rawPrelude.slice(0, boundary + 1)
    const prelude = rawPrelude.slice(boundary + 1)
    const header = prelude.trim()
    const body = css.slice(open + 1, close)
    output += leading

    if (header.startsWith('@')) {
      const nested =
        /^@(media|supports|container|layer|scope|document)\b/i.test(header)
      output += `${prelude}{${nested ? rewriteRuleList(body) : body}}`
    } else {
      const indentation = prelude.slice(0, prelude.indexOf(header))
      const scopedSelectors = splitSelectors(header)
        .map(scopeSelector)
        .join(',')
      output += `${indentation}${scopedSelectors}{${body}}`
    }
    cursor = close + 1
  }
  return output
}

function scopeSelector(selector: string): string {
  const value = selector.trim()
  if (!value || value.startsWith(scope)) return value
  const rootSelector = `${scope} ${dashboardRoot}`
  if (/^:root(?=$|[\s[.:#>+~])/i.test(value)) {
    return makeConsoleLintSafe(value.replace(/^:root/i, rootSelector))
  }
  if (/^(html|body)(?=$|[\s[.:#>+~])/i.test(value)) {
    return makeConsoleLintSafe(value.replace(/^(html|body)/i, rootSelector))
  }
  if (/^#root(?=$|[\s[.:#>+~])/i.test(value)) {
    return makeConsoleLintSafe(value.replace(/^#root/i, rootSelector))
  }
  return makeConsoleLintSafe(`${scope} ${value}`)
}

// The Console intentionally uses a cheap `selector.split(',')` lint. Prefix
// branches inside :is/:where/:not too, avoiding false global-style warnings.
function makeConsoleLintSafe(value: string): string {
  let output = ''
  let quote = ''
  let square = 0
  for (let index = 0; index < value.length; index++) {
    const char = value[index]
    output += char
    if (quote) {
      if (char === '\\') output += value[++index] ?? ''
      else if (char === quote) quote = ''
      continue
    }
    if (char === '"' || char === "'") quote = char
    else if (char === '[') square++
    else if (char === ']') square--
    else if (char === ',' && square === 0) output += `${scope} `
  }
  return output
}

function splitSelectors(value: string): string[] {
  const selectors: string[] = []
  let start = 0
  let round = 0
  let square = 0
  let quote = ''
  for (let index = 0; index < value.length; index++) {
    const char = value[index]
    if (quote) {
      if (char === '\\') index++
      else if (char === quote) quote = ''
      continue
    }
    if (char === '"' || char === "'") quote = char
    else if (char === '(') round++
    else if (char === ')') round--
    else if (char === '[') square++
    else if (char === ']') square--
    else if (char === ',' && round === 0 && square === 0) {
      selectors.push(value.slice(start, index).trim())
      start = index + 1
    }
  }
  selectors.push(value.slice(start).trim())
  return selectors
}

function findNextBrace(value: string, start: number): number {
  let quote = ''
  let comment = false
  for (let index = start; index < value.length; index++) {
    const char = value[index]
    const next = value[index + 1]
    if (comment) {
      if (char === '*' && next === '/') {
        comment = false
        index++
      }
      continue
    }
    if (quote) {
      if (char === '\\') index++
      else if (char === quote) quote = ''
      continue
    }
    if (char === '/' && next === '*') {
      comment = true
      index++
    } else if (char === '"' || char === "'") quote = char
    else if (char === '{') return index
  }
  return -1
}

function findMatchingBrace(value: string, open: number): number {
  let depth = 1
  let quote = ''
  let comment = false
  for (let index = open + 1; index < value.length; index++) {
    const char = value[index]
    const next = value[index + 1]
    if (comment) {
      if (char === '*' && next === '/') {
        comment = false
        index++
      }
      continue
    }
    if (quote) {
      if (char === '\\') index++
      else if (char === quote) quote = ''
      continue
    }
    if (char === '/' && next === '*') {
      comment = true
      index++
    } else if (char === '"' || char === "'") quote = char
    else if (char === '{') depth++
    else if (char === '}' && --depth === 0) return index
  }
  return -1
}

function lastTopLevelSemicolon(value: string): number {
  let last = -1
  let round = 0
  let square = 0
  let quote = ''
  for (let index = 0; index < value.length; index++) {
    const char = value[index]
    if (quote) {
      if (char === '\\') index++
      else if (char === quote) quote = ''
      continue
    }
    if (char === '"' || char === "'") quote = char
    else if (char === '(') round++
    else if (char === ')') round--
    else if (char === '[') square++
    else if (char === ']') square--
    else if (char === ';' && round === 0 && square === 0) last = index
  }
  return last
}

export default defineConfig({
  publicDir: false,
  plugins: [emptyFontAssets(), react(), tailwindcss(), scopedConsoleStyles()],
  resolve: {
    alias: {
      '@': path.resolve(root, 'src'),
    },
  },
  build: {
    outDir: 'dist-console',
    emptyOutDir: true,
    cssCodeSplit: false,
    lib: {
      entry: path.resolve(root, 'src/console-entry.tsx'),
      formats: ['es'],
      fileName: () => 'page.js',
    },
    rollupOptions: {
      external: [
        'react',
        'react-dom',
        'react-dom/client',
        'react/jsx-runtime',
        '@iii-dev/console-ui',
      ],
      output: {
        assetFileNames: (asset) =>
          asset.names.some((name) => name.endsWith('.css'))
            ? 'styles.css'
            : 'assets/[name]-[hash][extname]',
      },
    },
  },
})
