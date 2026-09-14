import type React from 'react'
import {
  type ButtonHTMLAttributes,
  createContext,
  type HTMLAttributes,
  type ReactNode,
  useContext,
  useState,
} from 'react'

export function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel = 'Continue',
  cancelLabel = 'Cancel',
  onConfirm,
  onCancel,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  title: string
  description?: ReactNode
  details?: readonly string[]
  confirmLabel?: string
  cancelLabel?: string
  onConfirm: () => void
  onCancel?: () => void
}) {
  if (!open) return null
  return (
    <dialog open data-ui="confirm-dialog">
      <h2>{title}</h2>
      {description}
      <button type="button" onClick={onCancel}>
        {cancelLabel}
      </button>
      <button type="button" onClick={onConfirm}>
        {confirmLabel}
      </button>
    </dialog>
  )
}

const CollapsibleContext = createContext<{
  open: boolean
  toggle(): void
}>({ open: false, toggle() {} })

export function CollapsibleCard({
  open,
  defaultOpen = false,
  onOpenChange,
  disabled: _disabled,
  children,
  ...props
}: HTMLAttributes<HTMLDivElement> & {
  open?: boolean
  defaultOpen?: boolean
  onOpenChange?(open: boolean): void
  disabled?: boolean
}) {
  const [internal, setInternal] = useState(defaultOpen)
  const current = open ?? internal
  return (
    <CollapsibleContext.Provider
      value={{
        open: current,
        toggle() {
          setInternal(!current)
          onOpenChange?.(!current)
        },
      }}
    >
      <div className="iii-ui-collapsible-card" data-open={current} {...props}>
        {children}
      </div>
    </CollapsibleContext.Provider>
  )
}

export function CollapsibleCardTrigger(
  props: ButtonHTMLAttributes<HTMLButtonElement>,
) {
  const card = useContext(CollapsibleContext)
  return (
    <button
      type="button"
      aria-expanded={card.open}
      {...props}
      onClick={() => card.toggle()}
    />
  )
}

export function CollapsibleCardContent(props: HTMLAttributes<HTMLElement>) {
  const card = useContext(CollapsibleContext)
  return <section hidden={!card.open} {...props} />
}

export function PageShell(props: HTMLAttributes<HTMLDivElement>) {
  return <div {...props} />
}

export function PageBody({
  side: _side,
  ...props
}: HTMLAttributes<HTMLDivElement> & { side?: 'left' | 'right' }) {
  return <div {...props} />
}

export function PageMain(props: HTMLAttributes<HTMLElement>) {
  return <main {...props} />
}

export function PageHeader({
  icon: _icon,
  title,
  description,
  actions,
  onClose: _onClose,
  children,
  ...props
}: HTMLAttributes<HTMLElement> & {
  icon?: ReactNode
  title?: ReactNode
  description?: ReactNode
  actions?: ReactNode
  onClose?: () => void
}) {
  return (
    <header {...props}>
      {title}
      {description}
      {children}
      {actions}
    </header>
  )
}

const TabsContext = createContext<{
  value?: string
  select(value: string): void
}>({ select() {} })

export function Tabs({
  value,
  defaultValue,
  onValueChange,
  children,
  ...props
}: HTMLAttributes<HTMLDivElement> & {
  value?: string
  defaultValue?: string
  onValueChange?(value: string): void
}) {
  const [internal, setInternal] = useState(defaultValue)
  return (
    <TabsContext.Provider
      value={{
        value: value ?? internal,
        select(next) {
          setInternal(next)
          onValueChange?.(next)
        },
      }}
    >
      <div {...props}>{children}</div>
    </TabsContext.Provider>
  )
}

export function TabsList(props: HTMLAttributes<HTMLDivElement>) {
  return <div role="tablist" {...props} />
}

export function TabsTrigger({
  value,
  icon: _icon,
  children,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  value: string
  icon?: ReactNode | false
}) {
  const tabs = useContext(TabsContext)
  return (
    <button
      type="button"
      role="tab"
      aria-selected={tabs.value === value}
      {...props}
      onClick={() => tabs.select(value)}
    >
      {children}
    </button>
  )
}

/* ---- primitives the pilot screens render; markup mirrors the host recipes ---- */

type Div = HTMLAttributes<HTMLDivElement>

export function Button({
  variant: _variant,
  size: _size,
  asChild,
  children,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: string
  size?: string
  asChild?: boolean
}) {
  if (asChild) return <>{children}</>
  return (
    <button type="button" {...props}>
      {children}
    </button>
  )
}

export function Input({
  value,
  onChange,
  preserveCase: _preserveCase,
  ...props
}: Omit<React.InputHTMLAttributes<HTMLInputElement>, 'onChange' | 'value'> & {
  value: string
  onChange: (next: string) => void
  preserveCase?: boolean
}) {
  return (
    <input
      value={value}
      onChange={(event) => onChange(event.target.value)}
      {...props}
    />
  )
}

export function Select<T extends string>({
  value,
  options = [],
  onChange,
  placeholder,
  ...props
}: {
  value: T | undefined
  options?: Array<{ value: T; label: string }>
  onChange: (next: T) => void
  placeholder?: string
  disabled?: boolean
  className?: string
  'aria-label'?: string
}) {
  return (
    <select
      value={value ?? ''}
      onChange={(event) => onChange(event.target.value as T)}
      {...props}
    >
      {placeholder ? <option value="">{placeholder}</option> : null}
      {options.map((option) => (
        <option key={option.value} value={option.value}>
          {option.label}
        </option>
      ))}
    </select>
  )
}

export function Skeleton(props: HTMLAttributes<HTMLSpanElement>) {
  return <span data-ui="skeleton" {...props} />
}

export function StatusPanel({
  variant = 'info',
  headline,
  detail,
  className,
}: {
  variant?: string
  icon?: ReactNode
  headline: ReactNode
  detail?: ReactNode
  className?: string
}) {
  return (
    <div data-ui="status-panel" data-variant={variant} className={className}>
      <strong>{headline}</strong>
      {detail}
    </div>
  )
}

export function EmptyState({
  title,
  description,
  action,
}: {
  icon?: unknown
  title: string
  description: string
  action?: { label: string; onClick: () => void }
}) {
  return (
    <section data-ui="empty-state">
      <h2>{title}</h2>
      <p>{description}</p>
      {action ? (
        <button type="button" onClick={action.onClick}>
          {action.label}
        </button>
      ) : null}
    </section>
  )
}

export function Badge({
  variant = 'default',
  children,
  ...props
}: HTMLAttributes<HTMLSpanElement> & { variant?: string }) {
  return (
    <span data-badge-variant={variant} {...props}>
      {children}
    </span>
  )
}

export function Chip({
  tone = 'neutral',
  selected,
  ...props
}: HTMLAttributes<HTMLSpanElement> & { tone?: string; selected?: boolean }) {
  return (
    <span
      className="iii-ui-chip"
      data-tone={tone === 'neutral' ? undefined : tone}
      data-selected={selected || undefined}
      {...props}
    />
  )
}

export function StatusDot({
  tone: _tone,
  pulse: _pulse,
  ...props
}: HTMLAttributes<HTMLSpanElement> & { tone?: string; pulse?: boolean }) {
  return <span data-ui="status-dot" {...props} />
}

export function TableViewport(props: Div) {
  return <div className="iii-ui-table-viewport" {...props} />
}
export function TableFrame(props: Div) {
  return <div className="iii-ui-table-frame" {...props} />
}
export function Table({
  density = 'comfortable',
  ...props
}: React.TableHTMLAttributes<HTMLTableElement> & { density?: string }) {
  return <table className="iii-ui-table" data-density={density} {...props} />
}
export function TableHeader(props: HTMLAttributes<HTMLTableSectionElement>) {
  return <thead className="iii-ui-table__header" {...props} />
}
export function TableBody(props: HTMLAttributes<HTMLTableSectionElement>) {
  return <tbody className="iii-ui-table__body" {...props} />
}
export function TableFooter(props: HTMLAttributes<HTMLTableSectionElement>) {
  return <tfoot className="iii-ui-table__footer" {...props} />
}
export function TableRow({
  interactive,
  selected,
  ...props
}: HTMLAttributes<HTMLTableRowElement> & {
  interactive?: boolean
  selected?: boolean
}) {
  return (
    <tr
      className="iii-ui-table__row"
      data-interactive={interactive || undefined}
      data-selected={selected || undefined}
      {...props}
    />
  )
}
export function TableHead(props: React.ThHTMLAttributes<HTMLTableCellElement>) {
  return <th className="iii-ui-table__head" {...props} />
}
export function TableCell(props: React.TdHTMLAttributes<HTMLTableCellElement>) {
  return <td className="iii-ui-table__cell" {...props} />
}
export function TableCaption(props: HTMLAttributes<HTMLTableCaptionElement>) {
  return <caption className="iii-ui-table__caption" {...props} />
}
