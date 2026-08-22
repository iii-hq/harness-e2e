import {
  type ButtonHTMLAttributes,
  createContext,
  type HTMLAttributes,
  type InputHTMLAttributes,
  type ReactNode,
  useContext,
  useState,
} from 'react'

type TabsContextValue = {
  value?: string
  onValueChange?: (value: string) => void
}

const TabsContext = createContext<TabsContextValue>({})

export function PageShell({
  className,
  children,
  ...props
}: HTMLAttributes<HTMLDivElement>) {
  return (
    <div className={className} {...props}>
      {children}
    </div>
  )
}

export function PageHeader({
  icon,
  title,
  description,
  actions,
  onClose,
  children,
  className,
}: {
  icon?: ReactNode
  title?: ReactNode
  description?: ReactNode
  actions?: ReactNode
  onClose?: () => void
  children?: ReactNode
  className?: string
}) {
  return (
    <header
      className={`harness-e2e-standalone-header flex min-h-14 items-center gap-2.5 border-b border-line bg-panel-raised px-4 ${className ?? ''}`}
    >
      <span className="harness-e2e-standalone-icon grid h-5 w-5 shrink-0 place-items-center text-brand">
        {icon}
      </span>
      <div className="harness-e2e-standalone-header-copy grid min-w-0 gap-0.5">
        <strong className="truncate font-mono text-xs font-semibold leading-tight text-ink">
          {title}
        </strong>
        <span className="truncate text-[11px] text-ink-soft">
          {description}
        </span>
      </div>
      {children}
      {actions}
      {onClose ? (
        <button
          className="ml-auto min-h-9 min-w-9 shrink-0 cursor-pointer border-0 bg-transparent text-lg text-ink-soft"
          type="button"
          onClick={onClose}
          aria-label="Close page"
        >
          ×
        </button>
      ) : null}
    </header>
  )
}

export function PageBody({
  className,
  children,
  ...props
}: HTMLAttributes<HTMLDivElement> & { side?: 'left' | 'right' }) {
  return (
    <div className={className} {...props}>
      {children}
    </div>
  )
}

export function PageMain({
  className,
  children,
  ...props
}: HTMLAttributes<HTMLElement>) {
  return (
    <main className={className} {...props}>
      {children}
    </main>
  )
}

export function Tabs({
  value,
  defaultValue,
  onValueChange,
  children,
  ...props
}: HTMLAttributes<HTMLDivElement> & {
  value?: string
  defaultValue?: string
  onValueChange?: (value: string) => void
}) {
  const [internal, setInternal] = useState(defaultValue)
  const current = value ?? internal
  const select = (next: string) => {
    setInternal(next)
    onValueChange?.(next)
  }
  return (
    <TabsContext.Provider value={{ value: current, onValueChange: select }}>
      <div {...props}>{children}</div>
    </TabsContext.Provider>
  )
}

export function TabsList({
  className,
  children,
  ...props
}: HTMLAttributes<HTMLDivElement>) {
  return (
    <div className={className} role="tablist" {...props}>
      {children}
    </div>
  )
}

export function TabsTrigger({
  value,
  className,
  children,
  // The standalone pills stay icon-free; the Console host renders this slot.
  icon: _icon,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  value: string
  icon?: ReactNode | false
}) {
  const context = useContext(TabsContext)
  const selected = context.value === value
  return (
    <button
      type="button"
      role="tab"
      aria-selected={selected}
      tabIndex={selected ? 0 : -1}
      className={className}
      onClick={() => context.onValueChange?.(value)}
      {...props}
    >
      {children}
    </button>
  )
}

export function Select<T extends string = string>({
  value,
  options = [],
  onChange,
  className,
  ...props
}: {
  value: T | undefined
  options?: Array<{ value: T; label: string }>
  onChange: (next: T) => void
  className?: string
  'aria-label'?: string
}) {
  return (
    <select
      value={value ?? ''}
      className={className}
      onChange={(event) => onChange(event.target.value as T)}
      {...props}
    >
      {options.map((option) => (
        <option key={option.value} value={option.value}>
          {option.label}
        </option>
      ))}
    </select>
  )
}

export function Button({
  children,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: string
  size?: string
  asChild?: boolean
}) {
  return (
    <button type="button" {...props}>
      {children}
    </button>
  )
}

export function Input({
  value,
  onChange,
  ...props
}: InputHTMLAttributes<HTMLInputElement> & {
  value: string
  onChange: (value: string) => void
}) {
  return (
    <input
      value={value}
      onChange={(event) => onChange(event.target.value)}
      {...props}
    />
  )
}

export function Skeleton({
  className,
  ...props
}: HTMLAttributes<HTMLSpanElement>) {
  return (
    <span
      className={`harness-e2e-skeleton block min-h-3 rounded-[4px] bg-panel-soft ${className ?? ''}`}
      {...props}
    />
  )
}

export function StatusPanel({
  headline,
  detail,
  className,
}: {
  headline: ReactNode
  detail?: ReactNode
  className?: string
}) {
  return (
    <section className={className}>
      <strong>{headline}</strong>
      {detail ? <p>{detail}</p> : null}
    </section>
  )
}

export function EmptyState({
  title,
  description,
  action,
}: {
  title: string
  description: string
  action?: { label: string; onClick: () => void }
}) {
  return (
    <section>
      <h2>{title}</h2>
      <p>{description}</p>
      {action ? <Button onClick={action.onClick}>{action.label}</Button> : null}
    </section>
  )
}

export function Dialog({ children }: { children?: ReactNode }) {
  return <>{children}</>
}

export function DropdownMenu({ children }: { children?: ReactNode }) {
  return <>{children}</>
}

export function PageSidebar({
  className,
  children,
  ...props
}: HTMLAttributes<HTMLElement> & { width?: number }) {
  return (
    <aside className={className} {...props}>
      {children}
    </aside>
  )
}
