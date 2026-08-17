import Form from '@rjsf/core'
import validator from '@rjsf/validator-ajv8'
import {
  Background,
  type Connection,
  Controls,
  type Edge,
  Handle,
  MiniMap,
  type Node,
  type NodeProps,
  Position,
  ReactFlow,
} from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import {
  Check,
  ChevronLeft,
  Copy,
  Download,
  FilePlus2,
  GitBranch,
  ListTree,
  Play,
  Plus,
  Redo2,
  Save,
  Trash2,
  Undo2,
  Upload,
} from 'lucide-react'
import {
  type ChangeEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react'
import { ThemeToggle } from '@/components/ThemeToggle'
import { hashForWorkflows, hashForWorkspace } from '@/hooks/use-hash-route'
import {
  canonicalWorkflowJson,
  type OfficialWorkflow,
  type StepTypeDescriptor,
  type WorkflowCatalog,
  type WorkflowDefinition,
  type WorkflowDraft,
  type WorkflowLayout,
  type WorkflowNodeDefinition,
  type WorkflowObservedStep,
  workflowDataSource,
} from '@/lib/workflow-data-source'

type EditorSnapshot = {
  definition: WorkflowDefinition
  layout: WorkflowLayout
}

type WorkflowNodeData = Record<string, unknown> & {
  definition: WorkflowNodeDefinition
  descriptor: StepTypeDescriptor | null
  observed: WorkflowObservedStep | null
  selected: boolean
}

function WorkflowGraphNode({ data }: NodeProps<Node<WorkflowNodeData>>) {
  const descriptor = data.descriptor
  const observed = data.observed
  return (
    <article
      className={`workflow-graph-node ${data.selected ? 'is-selected' : ''} ${observed ? `status-${observed.status}` : ''}`}
      aria-label={`${data.definition.id}, ${descriptor?.operational_kind ?? 'unknown'} step`}
    >
      {Object.entries(descriptor?.inputs ?? {}).map(
        ([port, definition], index) => (
          <Handle
            key={port}
            id={`input:${port}`}
            type="target"
            position={Position.Left}
            style={{ top: 44 + index * 18 }}
            title={`${port}: ${definition.kind}`}
          />
        ),
      )}
      <small>{descriptor?.operational_kind ?? 'unregistered'}</small>
      <strong>{data.definition.id}</strong>
      <span>
        {data.definition.step_type}@{data.definition.step_version}
      </span>
      {observed && (
        <footer>
          <b>{observed.status.replaceAll('_', ' ')}</b>
          <span>{observed.duration_ms} ms</span>
        </footer>
      )}
      {Object.entries(descriptor?.outputs ?? {}).map(
        ([port, definition], index) => (
          <Handle
            key={port}
            id={`output:${port}`}
            type="source"
            position={Position.Right}
            style={{ top: 44 + index * 18 }}
            title={`${port}: ${definition.kind}`}
          />
        ),
      )}
    </article>
  )
}

const nodeTypes = { workflow: WorkflowGraphNode }

function emptyWorkflow(): WorkflowDefinition {
  return {
    schema_version: 1,
    id: 'new.workflow',
    scenario_version: 1,
    description: 'Describe the sequential capability under evaluation.',
    limits: {
      max_parallel: 4,
      max_nodes: 64,
      step_timeout_seconds: 300,
      workflow_timeout_seconds: 1800,
      max_total_tokens: 100000,
      max_cost_usd: 10,
      technical_retries: 0,
    },
    nodes: [],
    criteria: [],
  }
}

function clone<T>(value: T): T {
  return structuredClone(value)
}

function descriptorFor(
  catalog: WorkflowCatalog | null,
  node: WorkflowNodeDefinition,
) {
  return (
    catalog?.step_types.find(
      (descriptor) =>
        descriptor.id === node.step_type &&
        descriptor.version === node.step_version,
    ) ?? null
  )
}

function defaultConfig(
  schema: Record<string, unknown>,
): Record<string, unknown> {
  const properties = (schema.properties ?? {}) as Record<
    string,
    Record<string, unknown>
  >
  return Object.fromEntries(
    Object.entries(properties)
      .filter(([, definition]) => 'default' in definition)
      .map(([key, definition]) => [key, definition.default]),
  )
}

function uniqueNodeId(
  definition: WorkflowDefinition,
  descriptor: StepTypeDescriptor,
) {
  const base = descriptor.id.replaceAll('.', '_').replaceAll('-', '_')
  let index = 1
  let candidate = base
  while (definition.nodes.some((node) => node.id === candidate)) {
    index += 1
    candidate = `${base}_${index}`
  }
  return candidate
}

function createsCycle(
  definition: WorkflowDefinition,
  source: string,
  target: string,
) {
  if (source === target) return true
  const children = new Map<string, string[]>()
  for (const node of definition.nodes) {
    for (const dependency of node.depends_on) {
      children.set(dependency, [...(children.get(dependency) ?? []), node.id])
    }
  }
  const pending = [target]
  const seen = new Set<string>()
  while (pending.length) {
    const current = pending.pop()
    if (!current || seen.has(current)) continue
    if (current === source) return true
    seen.add(current)
    pending.push(...(children.get(current) ?? []))
  }
  return false
}

function downloadDefinition(definition: WorkflowDefinition) {
  const blob = new Blob([canonicalWorkflowJson(definition)], {
    type: 'application/json;charset=utf-8',
  })
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = `${definition.id}.json`
  link.click()
  URL.revokeObjectURL(url)
}

export function WorkflowEditorPage({
  workflowId,
  executionId,
}: {
  workflowId: string | null
  executionId: string | null
}) {
  const [catalog, setCatalog] = useState<WorkflowCatalog | null>(null)
  const [draft, setDraft] = useState<WorkflowDraft | null>(null)
  const [sourceOfficial, setSourceOfficial] = useState<OfficialWorkflow | null>(
    null,
  )
  const [definition, setDefinition] =
    useState<WorkflowDefinition>(emptyWorkflow)
  const [layout, setLayout] = useState<WorkflowLayout>({})
  const [label, setLabel] = useState('New workflow')
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null)
  const [history, setHistory] = useState<EditorSnapshot[]>([])
  const [future, setFuture] = useState<EditorSnapshot[]>([])
  const [dirty, setDirty] = useState(false)
  const [saveState, setSaveState] = useState('Saved')
  const [error, setError] = useState('')
  const [validation, setValidation] = useState<string | null>(null)
  const [listMode, setListMode] = useState(false)
  const [run, setRun] = useState<{
    executionId: string
    status: string
    log: string
  } | null>(null)
  const [observed, setObserved] = useState<
    Record<string, WorkflowObservedStep>
  >({})
  const importRef = useRef<HTMLInputElement>(null)
  const saving = useRef(false)

  const loadDraft = useCallback((next: WorkflowDraft) => {
    setDraft(next)
    setSourceOfficial(null)
    setDefinition(clone(next.definition))
    setLayout(clone(next.layout ?? {}))
    setLabel(next.label)
    setHistory([])
    setFuture([])
    setDirty(false)
    setValidation(next.definition_sha256)
    setSelectedNodeId(null)
    window.history.replaceState(null, '', hashForWorkflows(next.id))
  }, [])

  const loadOfficial = useCallback((next: OfficialWorkflow) => {
    setDraft(null)
    setSourceOfficial(next)
    setDefinition(clone(next.definition))
    setLayout({})
    setLabel(next.id)
    setHistory([])
    setFuture([])
    setDirty(false)
    setValidation(next.definition_sha256)
    setSelectedNodeId(null)
    window.history.replaceState(null, '', hashForWorkflows(next.id))
  }, [])

  useEffect(() => {
    workflowDataSource
      .catalog()
      .then(async (loaded) => {
        setCatalog(loaded)
        if (executionId) {
          const detail = await workflowDataSource.runDetail(executionId)
          const workflowRun = detail.workflow_runs.at(-1)
          if (!workflowRun || !detail.workflow_definition) {
            throw new Error('Observed workflow definition is not available.')
          }
          const official = loaded.official.find(
            (item) => item.id === workflowRun.workflow_id,
          )
          setDraft(null)
          setSourceOfficial(official ?? null)
          setDefinition(clone(detail.workflow_definition))
          setLayout({})
          setLabel(`${workflowRun.workflow_id} · ${executionId}`)
          setHistory([])
          setFuture([])
          setDirty(false)
          setValidation(workflowRun.workflow_sha256 ?? null)
          setSelectedNodeId(null)
          setObserved(
            Object.fromEntries(
              workflowRun.steps.map((step) => [step.node_id, step]),
            ),
          )
          setRun({
            executionId,
            status:
              detail.passed === null
                ? 'running'
                : detail.passed
                  ? 'completed'
                  : 'failed',
            log: 'Read-only persisted workflow execution.',
          })
          return
        }
        const selectedDraft = loaded.drafts.find(
          (item) => item.id === workflowId,
        )
        const selectedOfficial = loaded.official.find(
          (item) => item.id === workflowId,
        )
        if (selectedDraft) loadDraft(selectedDraft)
        else if (selectedOfficial) loadOfficial(selectedOfficial)
        else if (loaded.drafts[0]) loadDraft(loaded.drafts[0])
        else if (loaded.official[0]) loadOfficial(loaded.official[0])
      })
      .catch((cause) => setError(String(cause)))
  }, [executionId, loadDraft, loadOfficial, workflowId])

  const commit = useCallback(
    (mutate: (next: EditorSnapshot) => void) => {
      if (!draft) return
      const previous = { definition: clone(definition), layout: clone(layout) }
      const next = clone(previous)
      mutate(next)
      setHistory((items) => [...items.slice(-49), previous])
      setFuture([])
      setDefinition(next.definition)
      setLayout(next.layout)
      setDirty(true)
      setValidation(null)
    },
    [definition, draft, layout],
  )

  const undo = useCallback(() => {
    const previous = history.at(-1)
    if (!draft || !previous) return
    setFuture((items) => [
      { definition: clone(definition), layout: clone(layout) },
      ...items,
    ])
    setHistory((items) => items.slice(0, -1))
    setDefinition(previous.definition)
    setLayout(previous.layout)
    setDirty(true)
  }, [definition, draft, history, layout])

  const redo = useCallback(() => {
    const next = future[0]
    if (!draft || !next) return
    setHistory((items) => [
      ...items,
      { definition: clone(definition), layout: clone(layout) },
    ])
    setFuture((items) => items.slice(1))
    setDefinition(next.definition)
    setLayout(next.layout)
    setDirty(true)
  }, [definition, draft, future, layout])

  const removeNode = useCallback(
    (id: string) => {
      commit((next) => {
        next.definition.nodes = next.definition.nodes
          .filter((node) => node.id !== id)
          .map((node) => ({
            ...node,
            depends_on: node.depends_on.filter(
              (dependency) => dependency !== id,
            ),
            inputs: Object.fromEntries(
              Object.entries(node.inputs).filter(
                ([, binding]) =>
                  binding.source !== 'output' || binding.node_id !== id,
              ),
            ),
          }))
        delete next.layout[id]
      })
      setSelectedNodeId(null)
    },
    [commit],
  )

  useEffect(() => {
    if (!dirty || !draft || saving.current) return
    const timeout = window.setTimeout(() => {
      saving.current = true
      setSaveState('Saving…')
      workflowDataSource
        .update(draft, label, definition, layout)
        .then((saved) => {
          setDraft(saved)
          setDirty(false)
          setSaveState('Saved')
        })
        .catch((cause) => {
          setError(String(cause))
          setSaveState('Save failed')
        })
        .finally(() => {
          saving.current = false
        })
    }, 700)
    return () => window.clearTimeout(timeout)
  }, [definition, dirty, draft, label, layout])

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const modifier = event.ctrlKey || event.metaKey
      if (modifier && event.key.toLowerCase() === 'z') {
        event.preventDefault()
        if (event.shiftKey) redo()
        else undo()
      }
      if (event.key === 'Delete' && selectedNodeId) {
        const active = document.activeElement?.tagName
        if (!['INPUT', 'TEXTAREA', 'SELECT'].includes(active ?? '')) {
          event.preventDefault()
          removeNode(selectedNodeId)
        }
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [redo, removeNode, selectedNodeId, undo])

  const observedById = observed
  const graphNodes = useMemo<Node<WorkflowNodeData>[]>(
    () =>
      definition.nodes.map((node, index) => ({
        id: node.id,
        type: 'workflow',
        position: layout[node.id] ?? {
          x: (index % 4) * 260,
          y: Math.floor(index / 4) * 180,
        },
        data: {
          definition: node,
          descriptor: descriptorFor(catalog, node),
          observed: observedById[node.id] ?? null,
          selected: selectedNodeId === node.id,
        },
      })),
    [catalog, definition.nodes, layout, observedById, selectedNodeId],
  )

  const graphEdges = useMemo<Edge[]>(() => {
    const edges: Edge[] = []
    for (const node of definition.nodes) {
      for (const dependency of node.depends_on) {
        edges.push({
          id: `dependency:${dependency}:${node.id}`,
          source: dependency,
          target: node.id,
          animated: observedById[node.id]?.status === 'running',
        })
      }
      for (const [input, binding] of Object.entries(node.inputs)) {
        if (binding.source !== 'output') continue
        edges.push({
          id: `binding:${binding.node_id}:${binding.port}:${node.id}:${input}`,
          source: binding.node_id,
          target: node.id,
          sourceHandle: `output:${binding.port}`,
          targetHandle: `input:${input}`,
          label: `${binding.port} → ${input}`,
        })
      }
    }
    return edges
  }, [definition.nodes, observedById])

  const onConnect = useCallback(
    (connection: Connection) => {
      if (!connection.source || !connection.target || !draft) return
      if (createsCycle(definition, connection.source, connection.target)) {
        setError('Connection rejected: it would create a cycle.')
        return
      }
      const source = definition.nodes.find(
        (node) => node.id === connection.source,
      )
      const target = definition.nodes.find(
        (node) => node.id === connection.target,
      )
      if (!source || !target) return
      const sourcePort = connection.sourceHandle?.replace('output:', '')
      const targetPort = connection.targetHandle?.replace('input:', '')
      if (sourcePort && targetPort) {
        const sourceDescriptor = descriptorFor(catalog, source)
        const targetDescriptor = descriptorFor(catalog, target)
        if (
          sourceDescriptor?.outputs[sourcePort]?.kind !==
          targetDescriptor?.inputs[targetPort]?.kind
        ) {
          setError('Connection rejected: port types are incompatible.')
          return
        }
      }
      commit((next) => {
        const node = next.definition.nodes.find(
          (candidate) => candidate.id === connection.target,
        )
        if (!node) return
        if (!node.depends_on.includes(connection.source)) {
          node.depends_on.push(connection.source)
          node.depends_on.sort()
        }
        if (sourcePort && targetPort) {
          node.inputs[targetPort] = {
            source: 'output',
            node_id: connection.source,
            port: sourcePort,
          }
        }
      })
      setError('')
    },
    [catalog, commit, definition, draft],
  )

  const addNode = (descriptor: StepTypeDescriptor) => {
    if (!draft) return
    const id = uniqueNodeId(definition, descriptor)
    commit((next) => {
      next.definition.nodes.push({
        id,
        step_type: descriptor.id,
        step_version: descriptor.version,
        config: defaultConfig(descriptor.config_schema),
        depends_on: [],
        inputs: {},
        activation: { policy: 'always' },
        dependency_policy: 'succeeded',
        required: true,
      })
      next.layout[id] = {
        x: (next.definition.nodes.length % 4) * 260,
        y: Math.floor(next.definition.nodes.length / 4) * 180,
      }
    })
    setSelectedNodeId(id)
  }

  const selectedNode = definition.nodes.find(
    (node) => node.id === selectedNodeId,
  )
  const selectedDescriptor = selectedNode
    ? descriptorFor(catalog, selectedNode)
    : null

  const saveAsDraft = async () => {
    try {
      const saved = await workflowDataSource.create(label, definition, layout)
      setCatalog((current) =>
        current ? { ...current, drafts: [saved, ...current.drafts] } : current,
      )
      loadDraft(saved)
    } catch (cause) {
      setError(String(cause))
    }
  }

  const validate = async () => {
    setError('')
    try {
      const result = await workflowDataSource.validate(definition)
      setValidation(result.definition_sha256)
    } catch (cause) {
      setValidation(null)
      setError(String(cause))
    }
  }

  const startRun = async () => {
    if (!draft) return
    try {
      const result = await workflowDataSource.validate(definition)
      if (result.definition_sha256 !== draft.definition_sha256 || dirty) {
        throw new Error(
          'Wait for autosave before executing the validated hash.',
        )
      }
      const identity = {
        url: localStorage.getItem('workflow.url') || 'ws://127.0.0.1:49134',
        model: localStorage.getItem('workflow.model') || '',
        provider: localStorage.getItem('workflow.provider') || '',
      }
      if (!identity.model || !identity.provider) {
        throw new Error(
          'Set workflow.model and workflow.provider in local storage before running.',
        )
      }
      const snapshot = await workflowDataSource.run(draft, identity)
      const executionId = snapshot.job?.id
      if (!executionId)
        throw new Error('Runner did not return an execution id.')
      setRun({ executionId, status: 'running', log: snapshot.job?.log ?? '' })
      setObserved({})
    } catch (cause) {
      setError(String(cause))
    }
  }

  useEffect(() => {
    if (!run || !['running', 'cancelling'].includes(run.status)) return
    const interval = window.setInterval(() => {
      workflowDataSource
        .runStatus(0)
        .then(async (snapshot) => {
          const job = snapshot.job
          if (!job || job.id !== run.executionId) return
          setRun({
            executionId: job.id,
            status: job.status,
            log: job.log ?? '',
          })
          try {
            const detail = await workflowDataSource.runDetail(job.id)
            const steps = detail.workflow_runs.at(-1)?.steps ?? []
            setObserved(
              Object.fromEntries(steps.map((step) => [step.node_id, step])),
            )
          } catch (cause) {
            if (job.status === 'completed') throw cause
          }
        })
        .catch((cause) => setError(String(cause)))
    }, 1000)
    return () => window.clearInterval(interval)
  }, [run])

  const importWorkflow = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0]
    if (!file) return
    try {
      const imported = JSON.parse(await file.text()) as WorkflowDefinition
      const saved = await workflowDataSource.create(imported.id, imported, {})
      setCatalog((current) =>
        current ? { ...current, drafts: [saved, ...current.drafts] } : current,
      )
      loadDraft(saved)
    } catch (cause) {
      setError(`Import failed: ${String(cause)}`)
    } finally {
      event.target.value = ''
    }
  }

  if (!catalog) {
    return (
      <main className="workflow-editor-empty">
        <a href={hashForWorkspace()}>
          <ChevronLeft size={16} /> Back
        </a>
        <h1>Workflow editor unavailable</h1>
        <p>{error || 'Loading the local workflow catalog…'}</p>
      </main>
    )
  }

  return (
    <main className="workflow-editor-shell">
      <header className="workflow-editor-header">
        <div>
          <a href={hashForWorkspace()} className="workflow-back-link">
            <ChevronLeft size={16} /> Executions
          </a>
          <span className="section-kicker">
            {executionId ? 'Observed workflow execution' : 'Local DAG composer'}
          </span>
          <h1>{executionId ? 'Executed DAG' : 'Workflow editor'}</h1>
          <p>
            {executionId
              ? 'Read-only node status, hard gates, assets, and failures from the persisted checkpoint and results.'
              : 'Compose only Rust-registered steps. Layout is saved separately and never changes the executable hash.'}
          </p>
        </div>
        <div className="workflow-header-actions">
          <span className="workflow-save-state">
            <Save size={14} /> {draft ? saveState : 'Read only'}
          </span>
          <ThemeToggle />
        </div>
      </header>

      {error && (
        <div className="workflow-error" role="alert">
          {error}
          <button type="button" onClick={() => setError('')}>
            Dismiss
          </button>
        </div>
      )}

      <section
        className="workflow-editor-toolbar"
        aria-label="Workflow actions"
      >
        <label>
          Name
          <input
            value={label}
            disabled={!draft}
            onChange={(event) => {
              setLabel(event.target.value)
              setDirty(true)
            }}
          />
        </label>
        <button
          type="button"
          onClick={() => {
            setDraft(null)
            setSourceOfficial(null)
            setDefinition(emptyWorkflow())
            setLayout({})
            setLabel('New workflow')
            setValidation(null)
          }}
        >
          <FilePlus2 size={16} /> New
        </button>
        <button type="button" onClick={saveAsDraft}>
          <Copy size={16} /> {draft ? 'Duplicate' : 'Save as draft'}
        </button>
        <button type="button" onClick={() => importRef.current?.click()}>
          <Upload size={16} /> Import
        </button>
        <input
          ref={importRef}
          type="file"
          accept="application/json,.json"
          hidden
          onChange={importWorkflow}
        />
        <button type="button" onClick={() => downloadDefinition(definition)}>
          <Download size={16} /> Export
        </button>
        <button type="button" disabled={!history.length} onClick={undo}>
          <Undo2 size={16} /> Undo
        </button>
        <button type="button" disabled={!future.length} onClick={redo}>
          <Redo2 size={16} /> Redo
        </button>
        <button type="button" onClick={() => setListMode((value) => !value)}>
          <ListTree size={16} /> {listMode ? 'Canvas' : 'Accessible list'}
        </button>
        <button type="button" className="workflow-validate" onClick={validate}>
          <Check size={16} /> Validate
        </button>
        <button
          type="button"
          className="workflow-run"
          disabled={!draft || dirty || validation !== draft.definition_sha256}
          onClick={startRun}
        >
          <Play size={16} /> Run validated hash
        </button>
      </section>

      <div className="workflow-editor-grid">
        <aside className="workflow-library" aria-label="Workflow library">
          <h2>Workflows</h2>
          <h3>Drafts</h3>
          {catalog.drafts.map((item) => (
            <button
              type="button"
              className={draft?.id === item.id ? 'is-active' : ''}
              key={item.id}
              onClick={() => loadDraft(item)}
            >
              <strong>{item.label}</strong>
              <span>{item.definition.id}</span>
            </button>
          ))}
          <h3>Official</h3>
          {catalog.official.map((item) => (
            <button
              type="button"
              className={sourceOfficial?.id === item.id ? 'is-active' : ''}
              key={item.id}
              onClick={() => loadOfficial(item)}
            >
              <strong>{item.id}</strong>
              <span>v{item.scenario_version}</span>
            </button>
          ))}
          <h2>Step catalog</h2>
          {catalog.step_types.map((descriptor) => (
            <button
              type="button"
              key={`${descriptor.id}@${descriptor.version}`}
              disabled={!draft}
              onClick={() => addNode(descriptor)}
              title={descriptor.description}
            >
              <Plus size={14} />
              <span>
                <strong>{descriptor.id}</strong>
                <small>{descriptor.operational_kind}</small>
              </span>
            </button>
          ))}
        </aside>

        <section className="workflow-canvas-panel">
          <header>
            <div>
              <strong>{definition.id}</strong>
              <span>{definition.nodes.length} nodes</span>
            </div>
            <code>{validation ?? 'not validated'}</code>
          </header>
          {listMode ? (
            <table className="workflow-node-table">
              <caption>Keyboard-accessible workflow node order</caption>
              <thead>
                <tr>
                  <th>Node</th>
                  <th>Type</th>
                  <th>Dependencies</th>
                  <th>Policy</th>
                  <th>Status</th>
                </tr>
              </thead>
              <tbody>
                {definition.nodes.map((node) => (
                  <tr key={node.id}>
                    <th>
                      <button
                        type="button"
                        onClick={() => setSelectedNodeId(node.id)}
                      >
                        {node.id}
                      </button>
                    </th>
                    <td>{node.step_type}</td>
                    <td>{node.depends_on.join(', ') || 'root'}</td>
                    <td>{node.dependency_policy}</td>
                    <td>{observed[node.id]?.status ?? 'not run'}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : (
            <div
              className="workflow-flow"
              role="application"
              aria-label="Workflow DAG canvas"
            >
              <ReactFlow
                nodes={graphNodes}
                edges={graphEdges}
                nodeTypes={nodeTypes}
                onConnect={onConnect}
                onNodeClick={(_, node) => setSelectedNodeId(node.id)}
                onNodeDragStop={(_, node) => {
                  if (!draft) return
                  setLayout((current) => ({
                    ...current,
                    [node.id]: node.position,
                  }))
                  setDirty(true)
                }}
                fitView
                minZoom={0.25}
              >
                <Background />
                <MiniMap pannable zoomable />
                <Controls />
              </ReactFlow>
            </div>
          )}
        </section>

        <aside className="workflow-inspector" aria-label="Workflow inspector">
          {selectedNode && selectedDescriptor ? (
            <>
              <header>
                <div>
                  <span className="section-kicker">Selected node</span>
                  <h2>{selectedNode.id}</h2>
                </div>
                <button
                  type="button"
                  disabled={!draft}
                  onClick={() => removeNode(selectedNode.id)}
                  aria-label={`Remove ${selectedNode.id}`}
                >
                  <Trash2 size={16} />
                </button>
              </header>
              <label>
                Node ID
                <input
                  disabled={!draft}
                  value={selectedNode.id}
                  onChange={(event) => {
                    const previous = selectedNode.id
                    const nextId = event.target.value
                    commit((next) => {
                      const node = next.definition.nodes.find(
                        (candidate) => candidate.id === previous,
                      )
                      if (!node) return
                      node.id = nextId
                      for (const candidate of next.definition.nodes) {
                        candidate.depends_on = candidate.depends_on.map(
                          (dependency) =>
                            dependency === previous ? nextId : dependency,
                        )
                      }
                      next.layout[nextId] = next.layout[previous]
                      delete next.layout[previous]
                    })
                    setSelectedNodeId(nextId)
                  }}
                />
              </label>
              <div className="workflow-node-flags">
                <label>
                  <input
                    type="checkbox"
                    checked={selectedNode.required}
                    disabled={
                      !draft || selectedNode.activation.policy !== 'always'
                    }
                    onChange={(event) =>
                      commit((next) => {
                        const node = next.definition.nodes.find(
                          (candidate) => candidate.id === selectedNode.id,
                        )
                        if (node) node.required = event.target.checked
                      })
                    }
                  />
                  Required
                </label>
                <label>
                  Dependency policy
                  <select
                    disabled={!draft}
                    value={selectedNode.dependency_policy}
                    onChange={(event) =>
                      commit((next) => {
                        const node = next.definition.nodes.find(
                          (candidate) => candidate.id === selectedNode.id,
                        )
                        if (node)
                          node.dependency_policy = event.target.value as
                            | 'succeeded'
                            | 'terminal'
                      })
                    }
                  >
                    <option value="succeeded">Succeeded</option>
                    <option value="terminal">Terminal join</option>
                  </select>
                </label>
              </div>
              <section>
                <h3>Typed configuration</h3>
                <Form
                  schema={selectedDescriptor.config_schema}
                  validator={validator}
                  formData={selectedNode.config}
                  disabled={!draft}
                  onChange={(change) =>
                    commit((next) => {
                      const node = next.definition.nodes.find(
                        (candidate) => candidate.id === selectedNode.id,
                      )
                      if (node)
                        node.config = (change.formData ?? {}) as Record<
                          string,
                          unknown
                        >
                    })
                  }
                />
              </section>
              <BranchEditor
                node={selectedNode}
                definition={definition}
                catalog={catalog}
                disabled={!draft}
                onChange={(activation) =>
                  commit((next) => {
                    const node = next.definition.nodes.find(
                      (candidate) => candidate.id === selectedNode.id,
                    )
                    if (node) {
                      node.activation = activation
                      if (activation.policy !== 'always') node.required = false
                    }
                  })
                }
              />
              {observed[selectedNode.id] && (
                <ObservedEvidence step={observed[selectedNode.id]} />
              )}
            </>
          ) : (
            <>
              <h2>Workflow settings</h2>
              <label>
                Workflow ID
                <input
                  value={definition.id}
                  disabled={!draft}
                  onChange={(event) =>
                    commit((next) => {
                      next.definition.id = event.target.value
                    })
                  }
                />
              </label>
              <label>
                Description
                <textarea
                  value={definition.description}
                  disabled={!draft}
                  onChange={(event) =>
                    commit((next) => {
                      next.definition.description = event.target.value
                    })
                  }
                />
              </label>
              <div className="workflow-budget-grid">
                {(
                  [
                    'max_parallel',
                    'max_nodes',
                    'step_timeout_seconds',
                    'workflow_timeout_seconds',
                    'technical_retries',
                  ] as const
                ).map((key) => (
                  <label key={key}>
                    {key.replaceAll('_', ' ')}
                    <input
                      type="number"
                      min={key === 'technical_retries' ? 0 : 1}
                      disabled={!draft}
                      value={definition.limits[key]}
                      onChange={(event) =>
                        commit((next) => {
                          next.definition.limits[key] = Number(
                            event.target.value,
                          )
                        })
                      }
                    />
                  </label>
                ))}
              </div>
              <label>
                Criteria JSON
                <textarea
                  className="workflow-code-input"
                  disabled={!draft}
                  defaultValue={JSON.stringify(definition.criteria, null, 2)}
                  onBlur={(event) => {
                    try {
                      const criteria = JSON.parse(event.target.value)
                      commit((next) => {
                        next.definition.criteria = criteria
                      })
                    } catch {
                      setError('Criteria must be valid JSON.')
                    }
                  }}
                />
              </label>
            </>
          )}
        </aside>
      </div>

      {run && (
        <section className="workflow-run-panel" aria-live="polite">
          <header>
            <div>
              <span className="section-kicker">Observed execution</span>
              <strong>{run.executionId}</strong>
            </div>
            <span className={`status-pill status-${run.status}`}>
              {run.status}
            </span>
          </header>
          <pre>{run.log || 'Waiting for runner output…'}</pre>
        </section>
      )}
    </main>
  )
}

function BranchEditor({
  node,
  definition,
  catalog,
  disabled,
  onChange,
}: {
  node: WorkflowNodeDefinition
  definition: WorkflowDefinition
  catalog: WorkflowCatalog
  disabled: boolean
  onChange: (activation: WorkflowNodeDefinition['activation']) => void
}) {
  const options = definition.nodes.flatMap((candidate) => {
    if (!node.depends_on.includes(candidate.id)) return []
    const descriptor = descriptorFor(catalog, candidate)
    return Object.entries(descriptor?.outputs ?? {})
      .filter(([, port]) => port.kind === 'boolean')
      .map(([port]) => ({ node_id: candidate.id, port }))
  })
  const condition =
    node.activation.policy === 'always' ? null : node.activation.conditions[0]
  return (
    <section className="workflow-branch-editor">
      <h3>
        <GitBranch size={15} /> Activation branch
      </h3>
      <label>
        Policy
        <select
          disabled={disabled}
          value={node.activation.policy}
          onChange={(event) => {
            const policy = event.target.value as 'always' | 'all' | 'any'
            if (policy === 'always') onChange({ policy })
            else {
              const first = options[0]
              onChange({
                policy,
                conditions: first ? [{ ...first, equals: true }] : [],
              })
            }
          }}
        >
          <option value="always">Always</option>
          <option value="all">All conditions</option>
          <option value="any">Any condition</option>
        </select>
      </label>
      {condition && (
        <>
          <label>
            Boolean output
            <select
              disabled={disabled}
              value={`${condition.node_id}.${condition.port}`}
              onChange={(event) => {
                const [node_id, port] = event.target.value.split('.')
                onChange({
                  policy: node.activation.policy as 'all' | 'any',
                  conditions: [{ node_id, port, equals: condition.equals }],
                })
              }}
            >
              {options.map((option) => (
                <option
                  key={`${option.node_id}.${option.port}`}
                  value={`${option.node_id}.${option.port}`}
                >
                  {option.node_id}.{option.port}
                </option>
              ))}
            </select>
          </label>
          <label>
            Equals
            <select
              disabled={disabled}
              value={String(condition.equals)}
              onChange={(event) =>
                onChange({
                  policy: node.activation.policy as 'all' | 'any',
                  conditions: [
                    { ...condition, equals: event.target.value === 'true' },
                  ],
                })
              }
            >
              <option value="true">true</option>
              <option value="false">false</option>
            </select>
          </label>
        </>
      )}
    </section>
  )
}

function ObservedEvidence({ step }: { step: WorkflowObservedStep }) {
  return (
    <section className="workflow-observed-evidence">
      <h3>Observed evidence</h3>
      <dl>
        <div>
          <dt>Status</dt>
          <dd>{step.status}</dd>
        </div>
        <div>
          <dt>Duration</dt>
          <dd>{step.duration_ms} ms</dd>
        </div>
        <div>
          <dt>Assets</dt>
          <dd>{step.assets?.length ?? 0}</dd>
        </div>
      </dl>
      {step.hard_gates?.map((gate) => (
        <p
          key={gate.id}
          className={gate.passed ? 'status-pass' : 'status-fail'}
        >
          <strong>{gate.id}</strong> {gate.reason}
        </p>
      ))}
      {step.evaluations?.map((evaluation) => (
        <p key={evaluation.id}>
          <strong>
            {evaluation.id} · {evaluation.outcome}
          </strong>{' '}
          {evaluation.summary}
        </p>
      ))}
      {step.failures?.map((failure, index) => (
        <p key={`${failure.phase}-${index}`} className="status-fail">
          <strong>{failure.phase}</strong> {failure.message}
        </p>
      ))}
      {step.assets?.map((asset) => (
        <code key={asset.id}>{asset.artifact.path}</code>
      ))}
    </section>
  )
}
