import { ChevronDown, X } from 'lucide-react'
import {
  type ButtonHTMLAttributes,
  type CSSProperties,
  type DialogHTMLAttributes,
  type HTMLAttributes,
  type InputHTMLAttributes,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
  type RefObject,
  type SelectHTMLAttributes,
  type TableHTMLAttributes,
  type TextareaHTMLAttributes,
  useEffect,
  useId,
  useRef,
} from 'react'

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

export type Breadcrumb = { label: string; href?: string }

export type PageHeaderProps = HTMLAttributes<HTMLElement> & {
  title: string
  summary: string
  headingLevel?: 1 | 2
  /** Id for the heading element, so a section can point aria-labelledby at it. */
  headingId?: string
  context?: string
  /** Trail above the title; the last entry is the current page. */
  breadcrumb?: Breadcrumb[]
  actions?: ReactNode
}

export function PageHeader({
  title,
  summary,
  headingLevel = 1,
  headingId,
  context,
  breadcrumb,
  actions,
  className,
  ...props
}: PageHeaderProps) {
  const Heading = headingLevel === 1 ? 'h1' : 'h2'
  return (
    <header className={classes('ds-page-header', className)} {...props}>
      <div className="ds-page-header-copy">
        {breadcrumb && breadcrumb.length > 0 ? (
          <nav className="ds-breadcrumb" aria-label="Breadcrumb">
            <ol>
              {breadcrumb.map((crumb, index) => {
                const last = index === breadcrumb.length - 1
                return (
                  <li key={`${crumb.label}:${crumb.href ?? ''}`}>
                    {crumb.href && !last ? (
                      <a href={crumb.href}>{crumb.label}</a>
                    ) : (
                      <span aria-current={last ? 'page' : undefined}>
                        {crumb.label}
                      </span>
                    )}
                  </li>
                )
              })}
            </ol>
          </nav>
        ) : null}
        {context ? <p className="ds-page-context">{context}</p> : null}
        <Heading id={headingId}>{title}</Heading>
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

/* ------------------------------------------------------------------ chips */

export type FilterChipProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  active?: boolean
  /** Number of rows this filter would show; rendered after the label. */
  count?: number | string
}

/** A toggle filter: pressed state is the fill, the count is part of the label. */
export function FilterChip({
  active = false,
  count,
  className,
  children,
  ...props
}: FilterChipProps) {
  return (
    <button
      className={classes('ds-chip', active && 'ds-chip-active', className)}
      type="button"
      aria-pressed={active}
      {...props}
    >
      <span>{children}</span>
      {count !== undefined ? (
        <span className="ds-chip-count">{count}</span>
      ) : null}
    </button>
  )
}

export function FilterChipGroup({
  label,
  className,
  children,
  ...props
}: HTMLAttributes<HTMLFieldSetElement> & { label: string }) {
  return (
    <fieldset className={classes('ds-chip-group', className)} {...props}>
      <legend className="ds-visually-hidden">{label}</legend>
      {children}
    </fieldset>
  )
}

/* ------------------------------------------------------------------ table */

export const numericCellClassName = 'ds-table-numeric'

export type DataTableProps = TableHTMLAttributes<HTMLTableElement> & {
  /** Read by assistive technology; shown only when captionVisible is set. */
  caption: string
  captionVisible?: boolean
  /** Header row sticks to the top of the nearest scroll container. */
  sticky?: boolean
  /** Below this width the wrapper scrolls horizontally instead of squeezing. */
  minWidth?: string
  /** In narrow containers rows become stacked cards labelled by data-label. */
  collapse?: boolean
  /** With collapse: the first cell is the card title, the rest wrap inline (audit T-03). */
  collapseInline?: boolean
  wrapClassName?: string
}

export function DataTable({
  caption,
  captionVisible = false,
  sticky = false,
  minWidth,
  collapse = false,
  collapseInline = false,
  wrapClassName,
  className,
  children,
  ...props
}: DataTableProps) {
  return (
    <div className={classes('ds-table-wrap', wrapClassName)}>
      <table
        className={classes(
          'ds-table',
          sticky && 'ds-table-sticky',
          collapse && 'ds-table-collapse',
          collapse && collapseInline && 'ds-table-collapse-inline',
          className,
        )}
        style={
          minWidth
            ? ({ '--ds-table-min-width': minWidth } as CSSProperties)
            : undefined
        }
        {...props}
      >
        <caption
          className={captionVisible ? 'ds-table-caption' : 'ds-visually-hidden'}
        >
          {caption}
        </caption>
        {children}
      </table>
    </div>
  )
}

/** True when a click landed on a control that handles itself. */
export function isInteractiveTarget(target: EventTarget | null) {
  return (
    target instanceof Element &&
    target.closest('a, button, input, select, textarea, summary, label') !==
      null
  )
}

export type DataTableRowProps = HTMLAttributes<HTMLTableRowElement> & {
  /** Hash route the whole row opens. Keep a real link in the first cell for the keyboard path. */
  href?: string
}

export function DataTableRow({
  href,
  className,
  onClick,
  children,
  ...props
}: DataTableRowProps) {
  const navigate = href
    ? (event: MouseEvent<HTMLTableRowElement>) => {
        onClick?.(event)
        if (event.defaultPrevented || isInteractiveTarget(event.target)) return
        window.location.hash = href
      }
    : onClick
  return (
    <tr
      className={classes(href && 'ds-table-row-link', className) || undefined}
      data-href={href}
      onClick={navigate}
      {...props}
    >
      {children}
    </tr>
  )
}

/* ------------------------------------------------------------ empty state */

export type EmptyStateProps = HTMLAttributes<HTMLElement> & {
  title: string
  description?: ReactNode
  actions?: ReactNode
  icon?: ReactNode
  headingLevel?: 2 | 3
  tone?: 'default' | 'error'
}

export function EmptyState({
  title,
  description,
  actions,
  icon,
  headingLevel = 2,
  tone = 'default',
  className,
  ...props
}: EmptyStateProps) {
  const Heading = headingLevel === 2 ? 'h2' : 'h3'
  return (
    <section
      className={classes(
        'ds-empty',
        tone === 'error' && 'ds-empty-error',
        className,
      )}
      role={tone === 'error' ? 'alert' : undefined}
      {...props}
    >
      {icon ? (
        <span className="ds-empty-icon" aria-hidden="true">
          {icon}
        </span>
      ) : null}
      <Heading className="ds-empty-title">{title}</Heading>
      {description ? (
        <p className="ds-empty-description">{description}</p>
      ) : null}
      {actions ? <div className="ds-empty-actions">{actions}</div> : null}
    </section>
  )
}

/* ------------------------------------------------------------------ delta */

export type DeltaDirection = 'up' | 'down' | 'flat' | 'unavailable'
export type DeltaTone = 'positive' | 'negative' | 'neutral' | 'unavailable'

export function deltaDirection(
  value: number | null | undefined,
): DeltaDirection {
  if (value == null || !Number.isFinite(value)) return 'unavailable'
  if (value > 0) return 'up'
  if (value < 0) return 'down'
  return 'flat'
}

export function deltaTone(
  direction: DeltaDirection,
  betterWhen: 'higher' | 'lower' | 'neither',
): DeltaTone {
  if (direction === 'unavailable') return 'unavailable'
  if (direction === 'flat' || betterWhen === 'neither') return 'neutral'
  const improved =
    betterWhen === 'higher' ? direction === 'up' : direction === 'down'
  return improved ? 'positive' : 'negative'
}

export type DeltaValueProps = HTMLAttributes<HTMLSpanElement> & {
  value: number | null | undefined
  /** Formats the magnitude, e.g. (v) => `${v.toFixed(1)}%`. */
  format?: (magnitude: number) => string
  /** Which direction counts as an improvement; colours follow it. */
  betterWhen?: 'higher' | 'lower' | 'neither'
  unavailableLabel?: string
}

/** A signed, directional delta: "+3.2% ▲" in the tone of the outcome. */
export function DeltaValue({
  value,
  format = (magnitude) => String(magnitude),
  betterWhen = 'neither',
  unavailableLabel = 'not reported',
  className,
  ...props
}: DeltaValueProps) {
  const direction = deltaDirection(value)
  const tone = deltaTone(direction, betterWhen)
  if (direction === 'unavailable') {
    return (
      <span
        className={classes('ds-delta', 'ds-delta-unavailable', className)}
        data-direction={direction}
        {...props}
      >
        <span aria-hidden="true">—</span>
        <span className="ds-visually-hidden">{unavailableLabel}</span>
      </span>
    )
  }
  const magnitude = format(Math.abs(value as number))
  const sign = direction === 'up' ? '+' : direction === 'down' ? '−' : '±'
  const glyph = direction === 'up' ? '▲' : direction === 'down' ? '▼' : null
  return (
    <span
      className={classes('ds-delta', `ds-delta-${tone}`, className)}
      data-direction={direction}
      {...props}
    >
      {sign}
      {magnitude}
      {glyph ? <span aria-hidden="true">{glyph}</span> : null}
      {tone === 'positive' || tone === 'negative' ? (
        <span className="ds-visually-hidden">
          {tone === 'positive' ? ', better' : ', worse'}
        </span>
      ) : null}
    </span>
  )
}

/* ---------------------------------------------------------------- callout */

export type CalloutTone = 'info' | 'success' | 'warning' | 'danger'

export type CalloutProps = HTMLAttributes<HTMLDivElement> & {
  tone?: CalloutTone
  title?: string
  icon?: ReactNode
}

const calloutRoles: Record<CalloutTone, 'note' | 'status' | 'alert'> = {
  info: 'note',
  success: 'status',
  warning: 'status',
  danger: 'alert',
}

export function Callout({
  tone = 'info',
  title,
  icon,
  className,
  children,
  ...props
}: CalloutProps) {
  return (
    <div
      className={classes('ds-callout', `ds-callout-${tone}`, className)}
      role={calloutRoles[tone]}
      {...props}
    >
      {icon ? (
        <span className="ds-callout-icon" aria-hidden="true">
          {icon}
        </span>
      ) : null}
      <div className="ds-callout-body">
        {title ? <strong className="ds-callout-title">{title}</strong> : null}
        <div>{children}</div>
      </div>
    </div>
  )
}

/* ------------------------------------------------------------------ field */

export type FieldProps = {
  label: ReactNode
  /** Id of the control inside; the hint and error ids derive from it. */
  htmlFor: string
  /** Short text at the end of the label row: "required", "optional", "automatic when blank". */
  meta?: ReactNode
  hint?: ReactNode
  error?: ReactNode
  className?: string
  children: ReactNode
}

/** Ids a control should list in aria-describedby for its Field. */
export function fieldDescribedBy(
  htmlFor: string,
  { hint = false, error = false }: { hint?: boolean; error?: boolean },
) {
  const ids = [
    hint ? `${htmlFor}-hint` : null,
    error ? `${htmlFor}-error` : null,
  ].filter(Boolean)
  return ids.length > 0 ? ids.join(' ') : undefined
}

export function Field({
  label,
  htmlFor,
  meta,
  hint,
  error,
  className,
  children,
}: FieldProps) {
  return (
    <div
      className={classes(
        'ds-field',
        error ? 'ds-field-invalid' : null,
        className,
      )}
    >
      <label className="ds-field-label" htmlFor={htmlFor}>
        <span>{label}</span>
        {meta ? <span className="ds-field-meta">{meta}</span> : null}
      </label>
      {children}
      {hint ? (
        <p className="ds-field-hint" id={`${htmlFor}-hint`}>
          {hint}
        </p>
      ) : null}
      {error ? (
        <p className="ds-field-error" id={`${htmlFor}-error`} role="alert">
          {error}
        </p>
      ) : null}
    </div>
  )
}

export function inputClassName(className?: string) {
  return classes('ds-input', className)
}

export function Input({
  className,
  ...props
}: InputHTMLAttributes<HTMLInputElement>) {
  return <input className={inputClassName(className)} {...props} />
}

export function Textarea({
  className,
  ...props
}: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea
      className={classes('ds-input', 'ds-textarea', className)}
      {...props}
    />
  )
}

export function Select({
  className,
  children,
  ...props
}: SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <span className="ds-select">
      <select
        className={classes('ds-input', 'ds-select-control', className)}
        {...props}
      >
        {children}
      </select>
      <span className="ds-select-chevron" aria-hidden="true">
        <ChevronDown size={14} />
      </span>
    </span>
  )
}

/* ----------------------------------------------------------------- dialog */

export type DialogSize = 'sm' | 'md' | 'lg' | 'xl'

export type DialogProps = Omit<
  DialogHTMLAttributes<HTMLDialogElement>,
  'title' | 'onClose'
> & {
  open: boolean
  /**
   * Called whenever closing is requested: Escape, a backdrop click, the close
   * control or a native close. The caller decides what happens (it may ask
   * for confirmation first) and drops `open` when it agrees.
   */
  onClose: () => void
  size?: DialogSize
  /** Fixed height (content scrolls inside); default height follows content. */
  tall?: boolean
  kicker?: ReactNode
  title: ReactNode
  description?: ReactNode
  /** Header content before the close control: badges, secondary actions. */
  actions?: ReactNode
  footer?: ReactNode
  closeLabel?: string
  bodyPadding?: boolean
  bodyClassName?: string
  /** Element to focus on open instead of the title (e.g. the safe action of a confirm). */
  initialFocus?: RefObject<HTMLElement | null>
  children?: ReactNode
}

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]:not([tabindex="-1"])'

/** Keeps Tab inside the dialog even where the browser would let it out. */
export function trapDialogFocus(event: KeyboardEvent<HTMLDialogElement>) {
  if (event.key !== 'Tab') return
  const focusable = [
    ...event.currentTarget.querySelectorAll<HTMLElement>(FOCUSABLE),
  ].filter((element) => !element.hidden && element.getClientRects().length > 0)
  if (focusable.length === 0) return
  const index = focusable.indexOf(document.activeElement as HTMLElement)
  const next = event.shiftKey
    ? index <= 0
      ? focusable.at(-1)
      : undefined
    : index === -1 || index === focusable.length - 1
      ? focusable[0]
      : undefined
  if (!next) return
  event.preventDefault()
  next.focus()
}

/**
 * The one modal: showModal(), focus on the title (or `initialFocus`), Escape
 * handled on the element (Chromium groups a modal opened from an Escape press
 * with its parent, so `cancel` may land elsewhere), backdrop click, header ·
 * scrolling body · footer, and a full-screen sheet on narrow viewports. No
 * shadow: the console strips them, so the backdrop carries the elevation.
 */
export function Dialog({
  open,
  onClose,
  size = 'md',
  tall = false,
  kicker,
  title,
  description,
  actions,
  footer,
  closeLabel = 'Close',
  bodyPadding = false,
  bodyClassName,
  initialFocus,
  className,
  children,
  ...props
}: DialogProps) {
  const ref = useRef<HTMLDialogElement>(null)
  const titleRef = useRef<HTMLHeadingElement>(null)
  const id = useId()
  const titleId = `${id}-title`
  const descriptionId = `${id}-description`

  useEffect(() => {
    const dialog = ref.current
    if (!dialog) return
    if (open && !dialog.open) {
      dialog.showModal()
      ;(initialFocus?.current ?? titleRef.current)?.focus()
    }
    if (!open && dialog.open) dialog.close()
  }, [open, initialFocus])

  return (
    <dialog
      ref={ref}
      className={classes(
        'ds-dialog',
        `ds-dialog-${size}`,
        tall && 'ds-dialog-tall',
        className,
      )}
      aria-labelledby={titleId}
      aria-describedby={description ? descriptionId : undefined}
      onCancel={(event) => {
        event.preventDefault()
        onClose()
      }}
      onClose={() => {
        if (open) onClose()
      }}
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          event.preventDefault()
          event.stopPropagation()
          onClose()
          return
        }
        trapDialogFocus(event)
      }}
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose()
      }}
      {...props}
    >
      <header className="ds-dialog-header">
        <div className="ds-dialog-heading">
          {kicker ? <span className="ds-label">{kicker}</span> : null}
          <h2
            id={titleId}
            ref={titleRef}
            tabIndex={-1}
            className="ds-dialog-title"
          >
            {title}
          </h2>
          {description ? (
            <p id={descriptionId} className="ds-dialog-description">
              {description}
            </p>
          ) : null}
        </div>
        <div className="ds-dialog-actions">
          {actions}
          <button
            className="ds-dialog-close"
            type="button"
            onClick={onClose}
            aria-label={closeLabel}
          >
            <X size={18} aria-hidden="true" />
          </button>
        </div>
      </header>
      {children ? (
        <div
          className={classes(
            'ds-dialog-body',
            bodyPadding && 'ds-dialog-body-padded',
            bodyClassName,
          )}
        >
          {children}
        </div>
      ) : null}
      {footer ? <footer className="ds-dialog-footer">{footer}</footer> : null}
    </dialog>
  )
}
