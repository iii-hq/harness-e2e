import { useEffect, useState } from 'react'

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

function loadScript(path: string, required = true) {
  return new Promise<boolean>((resolve, reject) => {
    const script = document.createElement('script')
    script.src = new URL(path, window.location.href).href
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

async function bootCoverage() {
  window.HARNESS_COVERAGE = undefined
  await loadScript('./coverage/summary.js', false)
  renderCoverage()
}

export function loadLegacyPage(page: 'coverage') {
  if (page !== 'coverage') {
    return Promise.reject(new Error(`Legacy page '${page}' was removed`))
  }
  window.__HARNESS_REACT_BOOT__ ??= {}
  window.__HARNESS_REACT_BOOT__.coverage ??= bootCoverage()
  return window.__HARNESS_REACT_BOOT__.coverage
}

export function useLegacyPage(page: 'coverage') {
  const [error, setError] = useState<Error | null>(null)

  useEffect(() => {
    let active = true
    loadLegacyPage(page).catch((cause: unknown) => {
      if (!active) return
      setError(cause instanceof Error ? cause : new Error(String(cause)))
    })
    return () => {
      active = false
    }
  }, [page])

  return error
}
