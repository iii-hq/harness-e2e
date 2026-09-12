import {
  type ButtonHTMLAttributes,
  createContext,
  type HTMLAttributes,
  type ReactNode,
  useContext,
  useState,
} from 'react'

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
