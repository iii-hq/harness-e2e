import type { ButtonHTMLAttributes, HTMLAttributes, ReactNode } from 'react'

function classes(...values: Array<string | false | null | undefined>) {
  return values.filter(Boolean).join(' ')
}

export type ButtonVariant = 'primary' | 'secondary' | 'quiet'
export type ButtonSize = 'compact' | 'default' | 'large'

export function buttonClassName({
  variant = 'secondary',
  size = 'default',
  className,
}: {
  variant?: ButtonVariant
  size?: ButtonSize
  className?: string
} = {}) {
  return classes(
    'ds-button',
    `ds-button-${variant}`,
    `ds-button-${size}`,
    className,
  )
}

export type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: ButtonVariant
  size?: ButtonSize
  busy?: boolean
}

export function Button({
  variant = 'secondary',
  size = 'default',
  busy = false,
  className,
  disabled,
  children,
  ...props
}: ButtonProps) {
  return (
    <button
      className={buttonClassName({ variant, size, className })}
      type="button"
      aria-busy={busy || undefined}
      disabled={disabled || busy}
      {...props}
    >
      {busy ? <span className="ds-button-spinner" aria-hidden="true" /> : null}
      <span>{children}</span>
    </button>
  )
}

export type PanelTone = 'default' | 'raised' | 'spotlight'
export type PanelPadding = 'none' | 'compact' | 'default' | 'generous'

export type PanelProps = HTMLAttributes<HTMLElement> & {
  as?: 'article' | 'div' | 'section'
  tone?: PanelTone
  padding?: PanelPadding
}

export function Panel({
  as: Component = 'section',
  tone = 'default',
  padding = 'default',
  className,
  ...props
}: PanelProps) {
  return (
    <Component
      className={classes(
        'ds-panel',
        `ds-panel-${tone}`,
        `ds-panel-padding-${padding}`,
        className,
      )}
      {...props}
    />
  )
}

export type OperationalStatus =
  | 'passed'
  | 'failed'
  | 'inconclusive'
  | 'unavailable'
  | 'hard_gate'
  | 'recommendation'
  | 'running'
  | 'cancelling'
  | 'cancelled'
  | 'incomplete'

const statusLabels: Record<OperationalStatus, string> = {
  passed: 'Passed',
  failed: 'Failed',
  inconclusive: 'Inconclusive',
  unavailable: 'Unavailable',
  hard_gate: 'Hard gate',
  recommendation: 'Recommendation',
  running: 'Running',
  cancelling: 'Cancelling',
  cancelled: 'Cancelled',
  incomplete: 'Incomplete',
}

export type StatusBadgeProps = HTMLAttributes<HTMLSpanElement> & {
  status: OperationalStatus
  label?: string
}

export function StatusBadge({
  status,
  label = statusLabels[status],
  className,
  ...props
}: StatusBadgeProps) {
  return (
    <span
      className={classes('ds-status-badge', `ds-status-${status}`, className)}
      data-status={status}
      {...props}
    >
      <span className="ds-status-dot" aria-hidden="true" />
      {label}
    </span>
  )
}

export type PageHeaderProps = HTMLAttributes<HTMLElement> & {
  title: string
  summary: string
  headingLevel?: 1 | 2
  context?: string
  actions?: ReactNode
}

export function PageHeader({
  title,
  summary,
  headingLevel = 1,
  context,
  actions,
  className,
  ...props
}: PageHeaderProps) {
  const Heading = headingLevel === 1 ? 'h1' : 'h2'
  return (
    <header className={classes('ds-page-header', className)} {...props}>
      <div className="ds-page-header-copy">
        {context ? <p className="ds-page-context">{context}</p> : null}
        <Heading>{title}</Heading>
        <p className="ds-page-summary">{summary}</p>
      </div>
      {actions ? <div className="ds-page-actions">{actions}</div> : null}
    </header>
  )
}

export type MetricTone =
  | 'neutral'
  | 'positive'
  | 'negative'
  | 'warning'
  | 'unavailable'

export type MetricCardProps = HTMLAttributes<HTMLElement> & {
  label: string
  value: string
  detail: string
  tone?: MetricTone
  delta?: string
}

export function MetricCard({
  label,
  value,
  detail,
  tone = 'neutral',
  delta,
  className,
  ...props
}: MetricCardProps) {
  return (
    <article
      className={classes('ds-metric-card', `ds-metric-${tone}`, className)}
      {...props}
    >
      <div className="ds-metric-heading">
        <span>{label}</span>
        {delta ? <strong>{delta}</strong> : null}
      </div>
      <div className="ds-metric-value">{value}</div>
      <p>{detail}</p>
    </article>
  )
}
