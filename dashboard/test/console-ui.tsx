import {
  type ButtonHTMLAttributes,
  cloneElement,
  createContext,
  type HTMLAttributes,
  isValidElement,
  type ReactElement,
  type ReactNode,
  type TableHTMLAttributes,
  type TdHTMLAttributes,
  type ThHTMLAttributes,
  useContext,
  useState,
} from 'react'

export function Tooltip({ children }: { children?: ReactNode }) {
  return <>{children}</>
}

export function TooltipTrigger({ children }: { children?: ReactNode }) {
  return <>{children}</>
}

export function TooltipContent() {
  return null
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

/* ---- the host's shared components; markup mirrors the host recipes ---- */

type Div = HTMLAttributes<HTMLDivElement>
type Trigger = ButtonHTMLAttributes<HTMLButtonElement> & { asChild?: boolean }

// Radix `asChild`: the child element becomes the trigger and gets its props.
function Slot({ asChild, children, ...props }: Trigger) {
  if (asChild && isValidElement(children)) {
    return cloneElement(children as ReactElement<Trigger>, props)
  }
  return (
    <button type="button" {...props}>
      {children}
    </button>
  )
}

export function Badge({
  variant = 'default',
  ...props
}: HTMLAttributes<HTMLSpanElement> & { variant?: string }) {
  return <span data-badge-variant={variant} {...props} />
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
  tone = 'accent',
  pulse,
  className,
  ...props
}: HTMLAttributes<HTMLSpanElement> & { tone?: string; pulse?: boolean }) {
  return (
    <span
      aria-hidden="true"
      data-ui="status-dot"
      data-tone={tone}
      className={[pulse && 'pulse-dot', className].filter(Boolean).join(' ')}
      {...props}
    />
  )
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

const DialogContext = createContext<{
  open: boolean
  setOpen(open: boolean): void
}>({ open: false, setOpen() {} })

export function Dialog({
  open,
  defaultOpen = false,
  onOpenChange,
  children,
}: {
  open?: boolean
  defaultOpen?: boolean
  onOpenChange?(open: boolean): void
  modal?: boolean
  children?: ReactNode
}) {
  const [internal, setInternal] = useState(defaultOpen)
  return (
    <DialogContext.Provider
      value={{
        open: open ?? internal,
        setOpen(next) {
          setInternal(next)
          onOpenChange?.(next)
        },
      }}
    >
      {children}
    </DialogContext.Provider>
  )
}

export function DialogTrigger(props: Trigger) {
  const dialog = useContext(DialogContext)
  return <Slot {...props} onClick={() => dialog.setOpen(true)} />
}

export function DialogClose(props: Trigger) {
  const dialog = useContext(DialogContext)
  return <Slot {...props} onClick={() => dialog.setOpen(false)} />
}

export function DialogContent({
  onOpenAutoFocus: _openFocus,
  onCloseAutoFocus: _closeFocus,
  onEscapeKeyDown: _escape,
  ...props
}: Div & {
  onOpenAutoFocus?(event: Event): void
  onCloseAutoFocus?(event: Event): void
  onEscapeKeyDown?(event: KeyboardEvent): void
}) {
  const dialog = useContext(DialogContext)
  if (!dialog.open) return null
  return <div role="dialog" {...props} />
}

export function DialogTitle(props: HTMLAttributes<HTMLHeadingElement>) {
  return <h2 {...props} />
}

export function DialogDescription(props: HTMLAttributes<HTMLParagraphElement>) {
  return <p {...props} />
}

export function ConfirmDialog({
  open,
  title,
  description,
  details,
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
    <div role="alertdialog" data-ui="confirm-dialog">
      <h2>{title}</h2>
      {description}
      {details?.length ? (
        <ul>
          {details.map((line) => (
            <li key={line}>{line}</li>
          ))}
        </ul>
      ) : null}
      <button type="button" onClick={onCancel}>
        {cancelLabel}
      </button>
      <button type="button" onClick={onConfirm}>
        {confirmLabel}
      </button>
    </div>
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
}: Div & {
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

// Radix renders a closed menu's content nowhere; the double keeps it in the
// markup, hidden, so static-markup tests can read the items.
const MenuContext = createContext<{
  open: boolean
  setOpen(open: boolean): void
}>({ open: false, setOpen() {} })

export function DropdownMenu({
  open,
  defaultOpen = false,
  onOpenChange,
  children,
}: {
  open?: boolean
  defaultOpen?: boolean
  onOpenChange?(open: boolean): void
  modal?: boolean
  children?: ReactNode
}) {
  const [internal, setInternal] = useState(defaultOpen)
  return (
    <MenuContext.Provider
      value={{
        open: open ?? internal,
        setOpen(next) {
          setInternal(next)
          onOpenChange?.(next)
        },
      }}
    >
      {children}
    </MenuContext.Provider>
  )
}

export function DropdownMenuTrigger(props: Trigger) {
  const menu = useContext(MenuContext)
  return (
    <Slot
      aria-haspopup="menu"
      aria-expanded={menu.open}
      data-state={menu.open ? 'open' : 'closed'}
      {...props}
      onClick={() => menu.setOpen(!menu.open)}
    />
  )
}

export function DropdownMenuContent({
  align: _align,
  side: _side,
  sideOffset: _sideOffset,
  alignOffset: _alignOffset,
  collisionPadding: _collisionPadding,
  loop: _loop,
  ...props
}: Div & {
  align?: string
  side?: string
  sideOffset?: number
  alignOffset?: number
  collisionPadding?: number
  loop?: boolean
}) {
  const menu = useContext(MenuContext)
  return <div role="menu" hidden={!menu.open} {...props} />
}

export function DropdownMenuItem({
  disabled,
  onSelect,
  textValue: _textValue,
  asChild: _asChild,
  ...props
}: Omit<Div, 'onSelect'> & {
  disabled?: boolean
  onSelect?(event: Event): void
  textValue?: string
  asChild?: boolean
}) {
  return (
    <div
      role="menuitem"
      tabIndex={-1}
      aria-disabled={disabled || undefined}
      data-disabled={disabled ? '' : undefined}
      {...props}
      onClick={disabled ? undefined : (click) => onSelect?.(click.nativeEvent)}
      onKeyDown={(key) => {
        if (!disabled && (key.key === 'Enter' || key.key === ' '))
          onSelect?.(key.nativeEvent)
      }}
    />
  )
}

export function DropdownMenuSeparator(props: HTMLAttributes<HTMLHRElement>) {
  return <hr {...props} />
}

export function DropdownMenuLabel(props: Div) {
  return <div {...props} />
}

export function DropdownMenuGroup(props: Div) {
  return <div {...props} />
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
}: TableHTMLAttributes<HTMLTableElement> & { density?: string }) {
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
export function TableHead(props: ThHTMLAttributes<HTMLTableCellElement>) {
  return <th className="iii-ui-table__head" {...props} />
}
export function TableCell(props: TdHTMLAttributes<HTMLTableCellElement>) {
  return <td className="iii-ui-table__cell" {...props} />
}
export function TableCaption(props: HTMLAttributes<HTMLTableCaptionElement>) {
  return <caption className="iii-ui-table__caption" {...props} />
}
