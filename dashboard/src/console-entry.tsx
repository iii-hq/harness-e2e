import type { Host, PageRenderProps } from '@iii-dev/console-ui'
import { App } from '@/App'
import { InvestigationContext } from '@/components/InvestigationAction'
import {
  installDashboardRuntimeConfig,
  type RuntimeConfig,
} from '@/lib/dashboard-data-source'
import { installDashboardIiiClient } from '@/lib/iii-client'
import './index.css'

const runtimeConfig: RuntimeConfig = {
  functions: {
    executions_list: 'e2e::dashboard::executions-list',
    execution_get: 'e2e::dashboard::execution-get',
    execution_delete: 'e2e::dashboard::execution-delete',
    execution_rename: 'e2e::dashboard::execution-rename',
    evidence_read: 'e2e::dashboard::evidence-read',
    github_runs_list: 'e2e::dashboard::github-runs-list',
    github_run_contracts: 'e2e::dashboard::github-run-contracts',
    github_run_import: 'e2e::dashboard::github-run-import',
    evaluated_versions_list: 'e2e::dashboard::evaluated-versions-list',
    tests_list: 'e2e::dashboard::tests-list',
    test_version_get: 'e2e::dashboard::test-version-get',
    test_history_get: 'e2e::dashboard::test-history-get',
    catalog_get: 'e2e::dashboard::catalog-get',
    execution_start: 'e2e::dashboard::execution-start',
    execution_slot_rerun: 'e2e::dashboard::execution-slot-rerun',
    execution_cancel: 'e2e::dashboard::execution-cancel',
    run_cancel: 'e2e::dashboard::run-cancel',
    suites_list: 'e2e::dashboard::suites-list',
    suite_create: 'e2e::dashboard::suite-create',
    suite_update: 'e2e::dashboard::suite-update',
    suite_delete: 'e2e::dashboard::suite-delete',
    stacks_list: 'e2e::dashboard::stacks-list',
    stack_create: 'e2e::dashboard::stack-create',
    stack_update: 'e2e::dashboard::stack-update',
    stack_delete: 'e2e::dashboard::stack-delete',
    credentials_list: 'e2e::dashboard::credentials-list',
    credential_set: 'e2e::dashboard::credential-set',
    credential_delete: 'e2e::dashboard::credential-delete',
    credentials_import: 'e2e::dashboard::credentials-import',
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
  return (
    <InvestigationContext value={host.chat?.openDraft}>
      <App
        tabId={tabId}
        panelSide={panelSide}
        theme={theme}
        onRequestClose={onRequestClose}
      />
    </InvestigationContext>
  )
}

export default function setup(host: Host) {
  installDashboardIiiClient(host.iii)
  installDashboardRuntimeConfig(runtimeConfig)
  const unregister = host.pages.register({
    id: 'harness-e2e',
    title: 'e2e',
    render: (props) => <DashboardPage host={host} {...props} />,
  })

  return () => {
    unregister()
  }
}
