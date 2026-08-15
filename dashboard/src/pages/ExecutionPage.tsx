import { LegacyLoadError } from '@/components/LegacyLoadError'
import { ThemeToggle } from '@/components/ThemeToggle'
import { hashForExecution, hashForWorkspace } from '@/hooks/use-hash-route'
import { useLegacyPage } from '@/hooks/useLegacyPage'

const evidenceKicker =
  'section-kicker mb-2 font-mono text-[0.61rem] font-semibold tracking-[0.055em] text-ink-muted'
const detailPanel =
  'detail-section scroll-mt-[82px] overflow-hidden rounded-[10px] border border-line-strong border-l-[3px] bg-panel px-7 py-[26px] max-[560px]:px-[18px] max-[560px]:py-[22px]'
const detailHeading =
  'm-0 text-[clamp(1.35rem,2vw,1.85rem)] font-[570] tracking-[-0.045em]'
const detailDisclosure =
  'section-disclosure -mx-7 -mb-[26px] mt-5 rounded-none border-0 border-t border-line bg-panel-quiet max-[560px]:-mx-[18px] max-[560px]:-mb-[22px]'
const disclosureSummary =
  'flex min-h-[54px] list-none items-center justify-between gap-4 px-7 py-3.5 max-[560px]:px-[18px]'
const detailKpi =
  'kpi-card min-h-[166px] rounded-none border-0 border-r border-b border-line bg-transparent p-[22px] max-[560px]:min-h-[138px] max-[560px]:border-r-0'
const detailKpiValue =
  'kpi-value mt-[26px] text-[clamp(2rem,3.2vw,3rem)] font-[540]'
const detailNavItem =
  'flex min-h-[52px] min-w-0 items-center gap-2.5 rounded-none border-r border-line px-4 text-left text-ink-muted no-underline hover:bg-panel-soft hover:text-ink focus-visible:bg-panel-soft focus-visible:text-ink focus-visible:outline-none max-[840px]:min-h-12 max-[840px]:px-3'

export function ExecutionPage({ executionId }: { executionId: string }) {
  const error = useLegacyPage('execution')

  return (
    <>
      <LegacyLoadError error={error} />
      <a className="skip-link" href={hashForExecution(executionId, 'main')}>
        Skip to execution details
      </a>
      <div className="ambient ambient-one hidden" aria-hidden="true"></div>
      <div className="ambient ambient-two hidden" aria-hidden="true"></div>

      <header className="topbar min-h-[68px]">
        <a
          className="brand"
          href="https://github.com/iii-hq/workers"
          aria-label="iii workers"
        >
          <span className="brand-copy">
            <strong>iii</strong>
            <span>Harness benchmarks</span>
          </span>
        </a>
        <nav className="topbar-actions" aria-label="Execution actions">
          <ThemeToggle />
          <a
            id="workflow-link"
            className="button button-secondary"
            href="https://github.com/iii-hq/workers/actions"
            data-mobile-label="Workflow"
          >
            Open workflow <span aria-hidden="true">↗</span>
          </a>
        </nav>
      </header>

      <main
        id="main"
        className="page-shell detail-shell w-[min(1420px,calc(100%-48px))] pt-[30px] max-[840px]:w-[min(1420px,calc(100%-30px))]"
      >
        <nav
          className="breadcrumbs mb-5 font-mono text-[0.64rem]"
          aria-label="Breadcrumb"
        >
          <a href={hashForWorkspace()}>Executions</a>
          <span aria-hidden="true">/</span>
          <span id="breadcrumb-run">Run</span>
        </nav>

        <section
          id="detail-loading"
          className="detail-loading"
          aria-live="polite"
        >
          Loading execution data…
        </section>

        <section id="detail-error" className="empty-state" hidden>
          <div className="empty-icon" aria-hidden="true">
            !
          </div>
          <h1>Execution not found</h1>
          <p id="detail-error-message">
            This execution is not present in the retained history.
          </p>
          <a className="button" href={hashForWorkspace()}>
            Back to executions
          </a>
        </section>

        <div id="detail-content" hidden>
          <section
            className="execution-summary grid grid-cols-[minmax(0,1.35fr)_minmax(420px,0.65fr)] overflow-hidden rounded-[10px] border border-line-strong border-t-[3px] border-t-brand bg-panel max-[1120px]:grid-cols-1"
            aria-labelledby="execution-heading"
          >
            <div className="execution-summary-main min-w-0 p-[30px] max-[560px]:px-[18px] max-[560px]:py-[22px]">
              <header className="execution-header grid content-start items-start justify-stretch gap-[18px] p-0">
                <div>
                  <div className="eyebrow mb-3">
                    <span className="live-dot" aria-hidden="true"></span>
                    Execution evidence
                  </div>
                  <div className="execution-title-row">
                    <h1
                      id="execution-heading"
                      className="grid max-w-full gap-3 text-[clamp(2rem,4vw,3.6rem)] leading-[0.98] font-[550] max-[560px]:text-[clamp(1.8rem,12vw,2.7rem)]"
                    >
                      <span>Execution</span>
                      <code
                        id="execution-title"
                        className="overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[clamp(0.82rem,1.35vw,1.05rem)] leading-[1.35] font-medium tracking-[-0.025em] text-ink-soft"
                      >
                        Run
                      </code>
                    </h1>
                  </div>
                  <p
                    id="execution-subtitle"
                    className="mt-3.5 text-ink-muted"
                  ></p>
                </div>
                <div className="execution-summary-meta flex flex-wrap items-center gap-2">
                  <span
                    id="execution-status"
                    className="status-pill status-incomplete"
                  >
                    Loading
                  </span>
                  <span id="availability-badge" className="data-badge">
                    Unknown data
                  </span>
                </div>
                <div
                  id="execution-actions"
                  className="execution-actions justify-start"
                ></div>
              </header>

              <section
                id="detail-failures"
                className="detail-alert mt-6 grid grid-cols-[minmax(190px,0.45fr)_minmax(0,1fr)] gap-6 rounded-none border-0 border-t border-t-[rgba(255,120,111,0.28)] bg-transparent pt-4 max-[560px]:grid-cols-1 [&_.failure-chip]:rounded-none [&_.failure-chip]:border-0 [&_.failure-chip]:border-b [&_.failure-chip]:border-line [&_.failure-chip]:bg-transparent [&_.failure-chip]:px-0 [&_.failure-chip]:py-2 [&_.failure-groups]:gap-1.5"
                hidden
                aria-labelledby="failures-title"
              >
                <div className="detail-alert-heading flex items-start gap-[11px]">
                  <span
                    className="latest-signal-icon text-danger"
                    aria-hidden="true"
                  >
                    !
                  </span>
                  <div>
                    <div className="section-kicker mb-[5px] text-danger">
                      Needs attention
                    </div>
                    <h2 id="failures-title" className="text-[0.84rem] text-ink">
                      Execution failures
                    </h2>
                  </div>
                </div>
                <div id="failure-summary"></div>
              </section>
            </div>

            <section
              className="kpi-grid detail-kpis m-0 grid grid-cols-3 gap-0 border-l border-line bg-panel-faint max-[1120px]:border-t max-[1120px]:border-l-0 max-[760px]:grid-cols-1"
              aria-label="Execution metrics"
            >
              <article
                className={`${detailKpi} kpi-primary bg-[radial-gradient(circle_at_100%_0,rgba(199,255,74,0.09),transparent_48%)]`}
              >
                <div className="kpi-label">Scenario pass rate</div>
                <div id="detail-pass-rate" className={detailKpiValue}>
                  —
                </div>
                <div id="detail-coverage" className="kpi-delta">
                  —
                </div>
                <div className="kpi-orbit" aria-hidden="true"></div>
              </article>
              <article
                className={`${detailKpi} border-b-0 max-[760px]:border-b`}
              >
                <div className="kpi-label">Model cost</div>
                <div id="detail-cost" className={detailKpiValue}>
                  —
                </div>
                <div className="kpi-delta">Subject and judge</div>
              </article>
              <article className={`${detailKpi} border-r-0 border-b-0`}>
                <div className="kpi-label">Model runtime</div>
                <div id="detail-runtime" className={detailKpiValue}>
                  —
                </div>
                <div id="workflow-runtime" className="kpi-delta">
                  —
                </div>
              </article>
            </section>
          </section>

          <div className="detail-layout mt-[18px] block">
            <nav
              className="detail-index sticky top-3 z-20 mb-4 grid grid-cols-4 gap-0 overflow-hidden rounded-[9px] border border-line-strong bg-glass p-0 shadow-[0_12px_30px_rgba(0,0,0,0.12)] backdrop-blur-[18px] max-[560px]:static max-[560px]:grid-cols-2"
              aria-label="Execution evidence sections"
            >
              <span className="visually-hidden">On this execution</span>
              <a
                href={hashForExecution(executionId, 'overview')}
                className={`${detailNavItem} max-[560px]:border-b`}
              >
                <span className="font-mono text-[0.6rem]" aria-hidden="true">
                  01
                </span>
                <strong className="overflow-hidden text-ellipsis whitespace-nowrap text-[0.74rem] font-semibold">
                  Context
                </strong>
              </a>
              <a
                href={hashForExecution(executionId, 'configuration')}
                className={`${detailNavItem} max-[560px]:border-r-0 max-[560px]:border-b`}
              >
                <span className="font-mono text-[0.6rem]" aria-hidden="true">
                  02
                </span>
                <strong className="overflow-hidden text-ellipsis whitespace-nowrap text-[0.74rem] font-semibold">
                  Stack
                </strong>
              </a>
              <a
                href={hashForExecution(executionId, 'scenarios')}
                className={detailNavItem}
              >
                <span className="font-mono text-[0.6rem]" aria-hidden="true">
                  03
                </span>
                <strong className="overflow-hidden text-ellipsis whitespace-nowrap text-[0.74rem] font-semibold">
                  Scenarios
                </strong>
              </a>
              <a
                href={hashForExecution(executionId, 'raw-data')}
                className={`${detailNavItem} border-r-0`}
              >
                <span className="font-mono text-[0.6rem]" aria-hidden="true">
                  04
                </span>
                <strong className="overflow-hidden text-ellipsis whitespace-nowrap text-[0.74rem] font-semibold">
                  Source
                </strong>
              </a>
            </nav>

            <div className="detail-main grid min-w-0 gap-4">
              <div className="detail-context-grid grid grid-cols-[minmax(0,0.85fr)_minmax(0,1.15fr)] gap-4 max-[1120px]:grid-cols-1">
                <section
                  id="overview"
                  className={`${detailPanel} detail-section-context border-l-info`}
                  aria-labelledby="overview-heading"
                >
                  <div className={evidenceKicker}>01 · Execution context</div>
                  <h2 id="overview-heading" className={detailHeading}>
                    Overview
                  </h2>
                  <details className={detailDisclosure}>
                    <summary className={disclosureSummary}>
                      <span
                        id="overview-digest"
                        className="section-digest overflow-hidden text-ellipsis whitespace-nowrap"
                      >
                        Loading…
                      </span>
                      <span className="section-chevron" aria-hidden="true">
                        ⌄
                      </span>
                    </summary>
                    <div
                      id="metadata-grid"
                      className="metadata-grid mt-0 grid grid-cols-2 gap-x-6 px-7 pb-6 max-[560px]:grid-cols-1 max-[560px]:px-[18px] max-[560px]:pb-[18px]"
                    ></div>
                  </details>
                </section>

                <section
                  id="configuration"
                  className={`${detailPanel} detail-section-configuration border-l-ink-muted`}
                  aria-labelledby="configuration-heading"
                >
                  <div className={evidenceKicker}>02 · Resolved stack</div>
                  <h2 id="configuration-heading" className={detailHeading}>
                    Configuration
                  </h2>
                  <details className={detailDisclosure}>
                    <summary className={disclosureSummary}>
                      <span
                        id="configuration-digest"
                        className="section-digest overflow-hidden text-ellipsis whitespace-nowrap"
                      >
                        Loading…
                      </span>
                      <span className="section-chevron" aria-hidden="true">
                        ⌄
                      </span>
                    </summary>
                    <div
                      id="configuration-content"
                      className="px-7 pb-6 max-[560px]:px-[18px] max-[560px]:pb-[18px]"
                    ></div>
                  </details>
                </section>
              </div>

              <section
                id="scenarios"
                className={`${detailPanel} detail-section-scenarios border-l-brand`}
                aria-labelledby="scenarios-heading"
              >
                <div className="detail-section-heading flex items-end justify-between gap-8 max-[840px]:flex-col max-[840px]:items-start max-[840px]:gap-2.5">
                  <div>
                    <div className={evidenceKicker}>
                      03 · Diagnostic workspace
                    </div>
                    <h2 id="scenarios-heading" className={detailHeading}>
                      Scenarios and runs
                    </h2>
                  </div>
                  <p
                    id="scenario-intro"
                    className="section-description m-0 max-w-[520px] text-right max-[840px]:text-left"
                  ></p>
                </div>
                <div
                  id="scenario-details"
                  className="scenario-details mt-[22px] grid gap-2"
                ></div>
              </section>

              <section
                id="raw-data"
                className={`${detailPanel} detail-section-raw border-l-line-strong bg-panel-faint`}
                aria-labelledby="raw-heading"
              >
                <div className={evidenceKicker}>04 · Source record</div>
                <h2 id="raw-heading" className={detailHeading}>
                  Raw data
                </h2>
                <p className="section-description">
                  The structured views above preserve the source report. Use the
                  raw record for fields that do not yet have a dedicated
                  visualization.
                </p>
                <div id="raw-actions" className="raw-actions"></div>
                <details
                  id="raw-preview"
                  className="raw-details border-t border-line pt-2.5"
                >
                  <summary>Preview JSON</summary>
                  <pre id="raw-json"></pre>
                </details>
              </section>
            </div>
          </div>
        </div>

        <dialog
          id="session-transcript-dialog"
          className="session-transcript-dialog h-[min(920px,calc(100dvh-24px))] w-[min(1120px,calc(100%-32px))] rounded-[10px] border border-line-strong border-t-[3px] border-t-info bg-panel shadow-panel backdrop:bg-app-backdrop backdrop:backdrop-blur-[5px]"
          aria-labelledby="session-transcript-title"
        >
          <div className="session-transcript-shell flex h-full min-h-0 flex-col">
            <header className="session-transcript-header flex items-start justify-between gap-6 border-b border-line bg-panel px-[26px] pt-[22px] pb-[18px]">
              <div>
                <div className={`${evidenceKicker} mb-[7px]`}>
                  Run evidence · Transcript
                </div>
                <h2
                  id="session-transcript-title"
                  className="m-0 text-[1.35rem] font-[570] tracking-[-0.025em]"
                >
                  Execution conversation
                </h2>
                <p id="session-transcript-context"></p>
              </div>
              <button
                id="session-transcript-close"
                className="dialog-close session-transcript-close bg-transparent"
                type="button"
                aria-label="Close session transcript"
              >
                ×
              </button>
            </header>
            <div
              id="session-transcript-body"
              className="session-transcript-body min-h-0 flex-1 overflow-hidden bg-panel [&_.conversation-shell]:bg-panel [&_.conversation-toolbar]:bg-panel-subtle"
            ></div>
          </div>
        </dialog>
      </main>

      <footer>
        <span>Harness E2E · public execution report</span>
        <a href={hashForWorkspace()}>Back to all executions</a>
      </footer>
    </>
  )
}
