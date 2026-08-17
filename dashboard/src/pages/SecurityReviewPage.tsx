import {
  Background,
  Controls,
  type Edge,
  Handle,
  type Node,
  type NodeProps,
  Position,
  ReactFlow,
} from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import { ChevronLeft, RefreshCw } from 'lucide-react'
import { type ReactNode, useEffect, useMemo, useState } from 'react'
import { ThemeToggle } from '@/components/ThemeToggle'
import { hashForExecution, hashForWorkspace } from '@/hooks/use-hash-route'
import {
  type SecurityReviewRunDetail,
  type SecurityReviewStep,
  securityReviewDataSource,
} from '@/lib/security-review-data-source'

type TestNodeData = Record<string, unknown> & { step: SecurityReviewStep }

function TestNode({ data }: NodeProps<Node<TestNodeData>>) {
  return (
    <article className={`security-review-node status-${data.step.status}`}>
      <Handle type="target" position={Position.Left} />
      <small>{data.step.required ? 'required test' : 'optional test'}</small>
      <strong>{humanize(data.step.node_id)}</strong>
      <span>{data.step.status.replaceAll('_', ' ')}</span>
      <footer>{formatDuration(data.step.duration_ms)}</footer>
      <Handle type="source" position={Position.Right} />
    </article>
  )
}

const nodeTypes = { test: TestNode }

export function SecurityReviewPage({ executionId }: { executionId: string }) {
  const [detail, setDetail] = useState<SecurityReviewRunDetail | null>(null)
  const [error, setError] = useState('')
  const [refreshing, setRefreshing] = useState(false)

  useEffect(() => {
    let active = true
    const load = async () => {
      setRefreshing(true)
      try {
        const next = await securityReviewDataSource.runDetail(executionId)
        if (active) {
          setDetail(next)
          setError('')
        }
      } catch (reason) {
        if (active)
          setError(reason instanceof Error ? reason.message : String(reason))
      } finally {
        if (active) setRefreshing(false)
      }
    }
    void load()
    const interval = window.setInterval(load, 2_000)
    return () => {
      active = false
      window.clearInterval(interval)
    }
  }, [executionId])

  const run = detail?.security_review_runs.at(-1)
  const graph = useMemo(() => graphFor(run?.steps ?? []), [run?.steps])

  return (
    <main className="security-review-page">
      <header className="security-review-header">
        <div>
          <a href={hashForWorkspace()} className="workflow-back-link">
            <ChevronLeft size={16} /> Dashboard
          </a>
          <p className="eyebrow">Rust-defined · read-only evidence</p>
          <h1>Security review</h1>
          <p>
            The scenario, advancement, skips and interruption policy are owned
            by Rust. This screen only projects persisted execution evidence.
          </p>
        </div>
        <div className="security-review-actions">
          <span
            className={`status-badge status-${detail?.passed === true ? 'succeeded' : detail?.passed === false ? 'failed' : 'running'}`}
          >
            {detail?.passed === true
              ? 'Passed'
              : detail?.passed === false
                ? 'Failed'
                : 'Running'}
          </span>
          <RefreshCw
            size={17}
            className={refreshing ? 'is-spinning' : ''}
            aria-label="Automatic refresh"
          />
          <ThemeToggle />
        </div>
      </header>

      {error && (
        <div className="workflow-error" role="alert">
          {error}
        </div>
      )}
      {!run ? (
        <section className="empty-state">
          <p>Waiting for the first persisted checkpoint…</p>
        </section>
      ) : (
        <>
          <section
            className="security-review-summary"
            aria-label="Execution identity"
          >
            <div>
              <small>Execution</small>
              <strong>{executionId}</strong>
            </div>
            <div>
              <small>Scenario</small>
              <strong>{run.workflow_id}</strong>
            </div>
            <div>
              <small>Flow hash</small>
              <strong>{shortHash(run.workflow_sha256)}</strong>
            </div>
            <div>
              <small>Cleanup hook</small>
              <strong>{run.cleanup?.status ?? 'pending'}</strong>
            </div>
          </section>

          <section
            className="security-review-canvas"
            aria-label="Read-only semantic test flow"
          >
            <ReactFlow
              nodes={graph.nodes}
              edges={graph.edges}
              nodeTypes={nodeTypes}
              nodesDraggable={false}
              nodesConnectable={false}
              elementsSelectable={false}
              minZoom={0.2}
              fitView
              proOptions={{ hideAttribution: true }}
            >
              <Background gap={22} size={1} />
              <Controls showInteractive={false} />
            </ReactFlow>
          </section>

          <section className="security-review-tests">
            <h2>Semantic tests</h2>
            <div className="security-review-test-list">
              {run.steps.map((step, index) => (
                <TestEvidence
                  key={step.node_id}
                  step={step}
                  number={index + 1}
                />
              ))}
            </div>
          </section>

          <section className="security-review-table-wrap">
            <table className="workflow-node-table">
              <caption>Accessible semantic test execution order</caption>
              <thead>
                <tr>
                  <th>Test</th>
                  <th>Status</th>
                  <th>Duration</th>
                  <th>Assets</th>
                  <th>Gates</th>
                  <th>Evaluations</th>
                </tr>
              </thead>
              <tbody>
                {run.steps.map((step) => (
                  <tr key={step.node_id}>
                    <th>{humanize(step.node_id)}</th>
                    <td>{step.status}</td>
                    <td>{formatDuration(step.duration_ms)}</td>
                    <td>{step.assets?.length ?? 0}</td>
                    <td>{step.hard_gates?.length ?? 0}</td>
                    <td>{step.evaluations?.length ?? 0}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </section>
        </>
      )}
      <footer className="security-review-footer">
        <a href={hashForExecution(executionId)}>
          Open complete execution evidence
        </a>
      </footer>
    </main>
  )
}

function TestEvidence({
  step,
  number,
}: {
  step: SecurityReviewStep
  number: number
}) {
  return (
    <article className="security-review-test-card">
      <header>
        <span>{String(number).padStart(2, '0')}</span>
        <div>
          <h3>{humanize(step.node_id)}</h3>
          <p>{step.step_type}</p>
        </div>
        <b className={`status-${step.status}`}>
          {step.status.replaceAll('_', ' ')}
        </b>
      </header>
      <dl>
        <div>
          <dt>Duration</dt>
          <dd>{formatDuration(step.duration_ms)}</dd>
        </div>
        <div>
          <dt>Metrics</dt>
          <dd>
            <pre>{formatJson(step.metrics)}</pre>
          </dd>
        </div>
      </dl>
      {step.skip_reason && (
        <p className="security-review-skip">
          <strong>Skip reason:</strong> {step.skip_reason}
        </p>
      )}
      <EvidenceGroup title="Assets" empty="No assets persisted.">
        {step.assets?.map((asset) => (
          <li key={asset.id}>
            <strong>{asset.id}</strong>
            <span>
              {asset.media_type ?? asset.kind} · {asset.size_bytes ?? 0} bytes
            </span>
            <code>{asset.artifact.path}</code>
          </li>
        ))}
      </EvidenceGroup>
      <EvidenceGroup title="Hard gates" empty="No hard gates reported.">
        {step.hard_gates?.map((gate) => (
          <li key={gate.id} className={gate.passed ? 'passed' : 'failed'}>
            <strong>
              {gate.passed ? 'PASS' : 'FAIL'} · {gate.id}
            </strong>
            <span>{gate.reason}</span>
          </li>
        ))}
      </EvidenceGroup>
      <EvidenceGroup title="Evaluations" empty="No evaluations reported.">
        {step.evaluations?.map((evaluation) => (
          <li key={evaluation.id}>
            <strong>
              {evaluation.outcome} · {evaluation.id}
            </strong>
            <span>{evaluation.summary}</span>
          </li>
        ))}
      </EvidenceGroup>
      <EvidenceGroup title="Failures" empty="No failures reported.">
        {step.failures?.map((failure) => (
          <li key={`${failure.phase}:${failure.message}`} className="failed">
            <strong>{failure.phase}</strong>
            <span>{failure.message}</span>
          </li>
        ))}
      </EvidenceGroup>
    </article>
  )
}

function EvidenceGroup({
  title,
  empty,
  children,
}: {
  title: string
  empty: string
  children?: ReactNode
}) {
  return (
    <section className="security-review-evidence">
      <h4>{title}</h4>
      {children ? <ul>{children}</ul> : <p>{empty}</p>}
    </section>
  )
}

function graphFor(steps: SecurityReviewStep[]): {
  nodes: Node<TestNodeData>[]
  edges: Edge[]
} {
  const depth = new Map<string, number>()
  for (const step of steps)
    depth.set(
      step.node_id,
      Math.max(
        0,
        ...(step.dependencies ?? []).map((id) => (depth.get(id) ?? 0) + 1),
      ),
    )
  const rows = new Map<number, number>()
  const nodes = steps.map((step) => {
    const level = depth.get(step.node_id) ?? 0
    const row = rows.get(level) ?? 0
    rows.set(level, row + 1)
    return {
      id: step.node_id,
      type: 'test',
      position: { x: level * 270, y: row * 150 },
      data: { step },
    }
  })
  const edges = steps.flatMap((step) =>
    (step.dependencies ?? []).map((dependency) => ({
      id: `${dependency}-${step.node_id}`,
      source: dependency,
      target: step.node_id,
      animated: step.status === 'running',
    })),
  )
  return { nodes, edges }
}

function humanize(value: string) {
  return value
    .replaceAll('_', ' ')
    .replace(/\b\w/g, (letter) => letter.toUpperCase())
}
function shortHash(value?: string) {
  return value ? `${value.slice(0, 18)}…` : 'pending'
}
function formatDuration(milliseconds: number) {
  return milliseconds < 1_000
    ? `${milliseconds} ms`
    : `${(milliseconds / 1_000).toFixed(1)} s`
}
function formatJson(value: unknown) {
  return value === undefined ? 'Not reported' : JSON.stringify(value, null, 2)
}
