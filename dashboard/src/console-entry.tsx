import {
  type Host,
  PageBody,
  PageHeader,
  PageMain,
  type PageRenderProps,
  PageShell,
} from '@iii-dev/console-ui'
import { useCallback } from 'react'
import { App } from '@/App'
import {
  installDashboardRuntimeConfig,
  type RuntimeConfig,
} from '@/lib/dashboard-data-source'
import { configureDashboardRuntime } from '@/lib/dashboard-runtime'
import { installDashboardIiiClient } from '@/lib/iii-client'
import './index.css'
import './console-overrides.css'

const HASH_BASE = '#/ext/harness-e2e'

const runtimeConfig: RuntimeConfig = {
  mode: 'local',
  transport: 'iii',
  page_size: 25,
  http_fallback: false,
  functions: {
    executions_list: 'e2e::dashboard::executions-list',
    execution_get: 'e2e::dashboard::execution-get',
    evaluated_versions_list: 'e2e::dashboard::evaluated-versions-list',
    tests_list: 'e2e::dashboard::tests-list',
    test_version_get: 'e2e::dashboard::test-version-get',
    test_history_get: 'e2e::dashboard::test-history-get',
    catalog_get: 'e2e::dashboard::catalog-get',
    run_status: 'e2e::dashboard::run-status',
    run_start: 'e2e::dashboard::run-start',
    run_cancel: 'e2e::dashboard::run-cancel',
    plans_list: 'e2e::dashboard::plans-list',
    plan_get: 'e2e::dashboard::plan-get',
    plan_create: 'e2e::dashboard::plan-create',
    plan_update: 'e2e::dashboard::plan-update',
    plan_run_start: 'e2e::dashboard::plan-run-start',
    changed_trigger: 'e2e::dashboard::changed',
  },
}

function HarnessE2eIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <path
        d="M2.5 4.25h11M2.5 8h7.25M2.5 11.75h11"
        stroke="currentColor"
        strokeWidth="1.35"
        strokeLinecap="round"
      />
      <circle cx="11.75" cy="8" r="1.5" fill="currentColor" />
    </svg>
  )
}

function DashboardPage({
  host,
  panelSide,
  onRequestClose,
}: PageRenderProps & { host: Host }) {
  const theme = host.useTheme()
  const handleInternalAnchor = useCallback(
    (event: React.MouseEvent<HTMLDivElement>) => {
      const link = (event.target as HTMLElement).closest<HTMLAnchorElement>(
        'a[href^="#"]',
      )
      const href = link?.getAttribute('href')
      if (!href || href.startsWith('#/')) return
      const id = decodeURIComponent(href.slice(1))
      const target = event.currentTarget.querySelector<HTMLElement>(
        `[id="${CSS.escape(id)}"]`,
      )
      if (!target) return
      event.preventDefault()
      target.scrollIntoView({ block: 'start' })
      target.focus({ preventScroll: true })
    },
    [],
  )

  return (
    <PageShell className="harness-e2e-console-shell">
      <PageHeader
        icon={<HarnessE2eIcon />}
        title="harness e2e"
        description="evidence, plans and live evaluation control"
        onClose={onRequestClose}
      />
      <PageBody side={panelSide}>
        <PageMain className="harness-e2e-console-main">
          <div
            className="harness-e2e-dashboard"
            data-harness-e2e-dashboard
            data-theme={theme}
            onClickCapture={handleInternalAnchor}
          >
            <App manageDocumentTitle={false} />
          </div>
        </PageMain>
      </PageBody>
    </PageShell>
  )
}

export default function setup(host: Host) {
  installDashboardIiiClient(host.iii)
  installDashboardRuntimeConfig(runtimeConfig)
  const restoreRuntime = configureDashboardRuntime({
    embedded: true,
    hashBase: HASH_BASE,
  })
  const unregister = host.pages.register({
    id: 'harness-e2e',
    title: 'e2e',
    render: (props) => <DashboardPage host={host} {...props} />,
  })

  return () => {
    unregister()
    restoreRuntime()
  }
}
