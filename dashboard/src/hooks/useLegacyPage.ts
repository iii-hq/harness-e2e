import { useEffect, useState } from 'react'
import { loadRuntimeExecutionData } from '@/lib/dashboard-data-source'

type PageName = 'overview' | 'execution' | 'coverage'

type CoverageLines = {
  count?: number
  covered?: number
  percent: number
}

type CoverageSuite = {
  generated_at?: string
  totals?: { lines?: CoverageLines }
}

type CoverageDocument = {
  updated_at?: string
  suites?: {
    e2e?: CoverageSuite
    integration?: CoverageSuite
  }
}

function assetRoot(_page: PageName) {
  return './'
}

function scriptUrl(page: PageName, path: string) {
  return new URL(`${assetRoot(page)}${path}`, window.location.href).href
}

function loadScript(page: PageName, path: string, required = true) {
  return new Promise<boolean>((resolve, reject) => {
    const script = document.createElement('script')
    script.src = scriptUrl(page, path)
    script.dataset.dashboardAsset = path
    script.onload = () => resolve(true)
    script.onerror = () => {
      script.remove()
      if (required) reject(new Error(`Unable to load ${path}`))
      else resolve(false)
    }
    document.head.append(script)
  })
}

async function loadExecutionManifest(page: PageName) {
  window.HARNESS_EXECUTIONS ??= undefined
  await loadScript(page, 'executions.js', false)
  if (!window.HARNESS_EXECUTIONS) {
    if (!window.HarnessBenchmarkData) {
      await loadBenchmarkData(page)
      await loadScript(page, 'dashboard-data.js')
    }
    await loadScript(page, 'sample-executions.js')
  }
}

async function loadExecutionData(page: 'overview' | 'execution') {
  try {
    if (await loadRuntimeExecutionData(page)) return
  } catch {
    // The local iii surface and its HTTP fallback may both be unavailable.
    // Retained/static data remains a valid read-only source.
  }
  await loadExecutionManifest(page)
}

async function loadBenchmarkData(page: PageName) {
  window.BENCHMARK_DATA ??= undefined
  await loadScript(page, 'data.js', false)
  if (!window.BENCHMARK_DATA) {
    await loadScript(page, 'sample-data.js')
  }
}

function renderCoverage() {
  const data = window.HARNESS_COVERAGE as CoverageDocument | undefined
  const empty = document.querySelector<HTMLElement>('#empty-state')
  const content = document.querySelector<HTMLElement>('#coverage-content')
  const suites = data?.suites ?? {}

  if (!suites.e2e && !suites.integration) {
    if (empty) empty.hidden = false
    return
  }
  if (content) content.hidden = false

  if (data?.updated_at) {
    const time = document.querySelector<HTMLTimeElement>('#last-update')
    if (time) {
      time.dateTime = data.updated_at
      time.textContent = new Date(data.updated_at).toLocaleString()
    }
  }

  const renderCard = (id: string, summary?: CoverageSuite) => {
    const lines = summary?.totals?.lines
    const card = document.querySelector<HTMLElement>(`#${id}`)
    if (!card || !lines) return
    const percent = card.querySelector<HTMLElement>('[data-percent]')
    const meta = card.querySelector<HTMLElement>('[data-meta]')
    const report = card.querySelector<HTMLElement>('[data-report]')
    if (percent) percent.textContent = `${lines.percent.toFixed(1)}%`
    if (meta) {
      meta.textContent = `${lines.covered?.toLocaleString() ?? '?'} of ${lines.count?.toLocaleString() ?? '?'} lines covered · ${summary.generated_at ?? ''}`
    }
    if (report) report.hidden = false
  }

  renderCard('card-integration', suites.integration)
  renderCard('card-e2e', suites.e2e)
}

async function bootLegacyPage(page: PageName) {
  if (page === 'coverage') {
    window.HARNESS_COVERAGE = undefined
    await loadScript(page, 'coverage/summary.js', false)
    renderCoverage()
    return
  }

  await loadBenchmarkData(page)
  await loadScript(page, 'dashboard-data.js')
  await loadScript(page, 'execution-data.js')
  if (page === 'execution') {
    await loadScript(page, 'execution-transcript.js')
  }
  await loadExecutionData(page)

  if (page === 'overview' && window.HARNESS_EXECUTIONS?.mode === 'local') {
    await loadScript(page, 'ansi-log.js')
    await loadScript(page, 'local-runner.js')
  }
  await loadScript(page, page === 'overview' ? 'overview.js' : 'execution.js')
}

function boot(page: PageName) {
  window.__HARNESS_REACT_BOOT__ ??= {}
  window.__HARNESS_REACT_BOOT__[page] ??= bootLegacyPage(page)
  return window.__HARNESS_REACT_BOOT__[page]
}

export function useLegacyPage(page: PageName) {
  const [error, setError] = useState<Error | null>(null)

  useEffect(() => {
    let active = true
    boot(page).catch((cause: unknown) => {
      if (!active) return
      setError(cause instanceof Error ? cause : new Error(String(cause)))
    })
    return () => {
      active = false
    }
  }, [page])

  return error
}
