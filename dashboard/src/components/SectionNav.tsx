import {
  Activity,
  ChartNoAxesCombined,
  GitCompare,
  History,
  ListTree,
} from 'lucide-react'

const sections = [
  { href: '#latest-evidence', label: 'Health', Icon: Activity },
  { href: '#comparison', label: 'Compare', Icon: GitCompare },
  { href: '#capability', label: 'Capability', Icon: ListTree },
  { href: '#efficiency', label: 'Efficiency', Icon: ChartNoAxesCombined },
  { href: '#executions', label: 'Executions', Icon: History },
]

export function SectionNav() {
  return (
    <nav className="section-nav" aria-label="Workspace sections">
      {sections.map(({ href, label, Icon }) => (
        <a key={href} href={href}>
          <Icon size={15} strokeWidth={1.8} aria-hidden="true" />
          {label}
        </a>
      ))}
    </nav>
  )
}
