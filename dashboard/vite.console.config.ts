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
        const scoped = scopeCss(renameKeyframes(css))
        if (!scoped.includes(scope)) {
          throw new Error(
            `injectable console CSS is missing its ${scope} scope`,
          )
        }
        writeFileSync(outputPath, scoped)
      }
    },
  }
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
