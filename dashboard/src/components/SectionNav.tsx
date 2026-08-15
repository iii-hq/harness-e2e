import type { WorkspaceView } from '@/hooks/use-hash-route'

const views: Array<{
  id: WorkspaceView
  index: string
  label: string
  description: string
}> = [
  {
    id: 'overview',
    index: '01',
    label: 'Overview',
    description: 'Current signal and regression gate',
  },
  {
    id: 'scenarios',
    index: '02',
    label: 'Scenarios',
    description: 'Outcome matrix and scenario economics',
  },
  {
    id: 'capability',
    index: '03',
    label: 'Capability',
    description: 'Evidence-backed complexity frontier',
  },
  {
    id: 'executions',
    index: '04',
    label: 'Executions',
    description: 'Immutable run ledger',
  },
]

type SectionNavProps = {
  activeView: WorkspaceView
  onViewChange: (view: WorkspaceView) => void
}

export function SectionNav({ activeView, onViewChange }: SectionNavProps) {
  return (
    <nav className="section-nav" aria-label="Evidence workspace views">
      {views.map(({ id, index, label, description }) => (
        <button
          key={id}
          id={`workspace-view-${id}`}
          className={activeView === id ? 'active' : undefined}
          type="button"
          aria-current={activeView === id ? 'page' : undefined}
          title={description}
          onClick={() => onViewChange(id)}
        >
          <span aria-hidden="true">{index}</span>
          <strong>{label}</strong>
        </button>
      ))}
    </nav>
  )
}
