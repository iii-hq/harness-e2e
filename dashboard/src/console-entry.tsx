import type { Host, PageRenderProps } from '@iii-dev/console-ui'
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

type ChatCapableHost = Host & {
  chat?: { selectConversation?: (sessionId: string) => void }
}

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

function DashboardPage({
  host,
  panelSide,
  tabId,
  onRequestClose,
}: PageRenderProps & { host: Host }) {
  const theme = host.useTheme()
  const chat = (host as ChatCapableHost).chat
  return (
    <App
      embedded
      tabId={tabId}
      panelSide={panelSide}
      theme={theme}
      openChat={
        chat?.selectConversation
          ? (sessionId) => chat.selectConversation?.(sessionId)
          : undefined
      }
      onRequestClose={onRequestClose}
      manageDocumentTitle={false}
    />
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
