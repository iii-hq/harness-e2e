import {
  type ButtonHTMLAttributes,
  cloneElement,
  createContext,
  type HTMLAttributes,
  isValidElement,
  type ReactElement,
  type ReactNode,
  type Ref,
  type RefObject,
  type TableHTMLAttributes,
  type TdHTMLAttributes,
  type ThHTMLAttributes,
  useContext,
  useEffect,
  useRef,
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
type Trigger = ButtonHTMLAttributes<HTMLButtonElement> & {
  asChild?: boolean
  ref?: Ref<HTMLButtonElement>
}

// Radix composeEventHandlers: the caller's handler runs first, the
// component's own only if the caller did not prevent the default.
function compose<E extends { defaultPrevented: boolean }>(
  theirs: ((event: E) => void) | undefined,
  ours: ((event: E) => void) | undefined,
) {
  return (event: E) => {
    theirs?.(event)
    if (!event.defaultPrevented) ours?.(event)
  }
}

// Radix `asChild`: the child element becomes the trigger, handlers composed.
function Slot({ asChild, children, onClick, onKeyDown, ...props }: Trigger) {
  if (asChild && isValidElement(children)) {
    const child = children as ReactElement<Trigger>
    return cloneElement(child, {
      ...props,
      onClick: compose(child.props.onClick, onClick),
      onKeyDown: compose(child.props.onKeyDown, onKeyDown),
    })
  }
  return (
    <button type="button" {...props} onClick={onClick} onKeyDown={onKeyDown}>
      {children}
    </button>
  )
}

/** Closes on a pointer down outside `inside`, like Radix's dismissable layer. */
function useOutsidePointer(
  open: boolean,
  inside: Array<RefObject<HTMLElement | null>>,
  close: () => void,
) {
  useEffect(() => {
    if (!open) return
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node
      if (!inside.some((ref) => ref.current?.contains(target))) close()
    }
    document.addEventListener('pointerdown', onPointerDown)
    return () => document.removeEventListener('pointerdown', onPointerDown)
  })
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
      className={[
        'inline-block size-1.5 rounded-full shrink-0',
        `bg-${tone}`,
        pulse && 'pulse-dot',
        className,
      ]
        .filter(Boolean)
        .join(' ')}
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

export function DialogTrigger({ onClick, ...props }: Trigger) {
  const dialog = useContext(DialogContext)
  return (
    <Slot {...props} onClick={compose(onClick, () => dialog.setOpen(true))} />
  )
}

export function DialogClose({ onClick, ...props }: Trigger) {
  const dialog = useContext(DialogContext)
  return (
    <Slot {...props} onClick={compose(onClick, () => dialog.setOpen(false))} />
  )
}

/** Closes on Escape and on the overlay, as Radix does. */
export function DialogContent({
  onOpenAutoFocus: _openFocus,
  onCloseAutoFocus: _closeFocus,
  onEscapeKeyDown,
  onKeyDown,
  ...props
}: Div & {
  onOpenAutoFocus?(event: Event): void
  onCloseAutoFocus?(event: Event): void
  onEscapeKeyDown?(event: KeyboardEvent): void
}) {
  const dialog = useContext(DialogContext)
  if (!dialog.open) return null
  return (
    <>
      {/* biome-ignore lint/a11y/noStaticElementInteractions lint/a11y/useKeyWithClickEvents: the host overlay closes on a pointer; Escape is on the content */}
      <div data-overlay="" onClick={() => dialog.setOpen(false)} />
      <div
        role="dialog"
        {...props}
        onKeyDown={compose(onKeyDown, (key) => {
          if (key.key !== 'Escape') return
          onEscapeKeyDown?.(key.nativeEvent)
          if (!key.nativeEvent.defaultPrevented) dialog.setOpen(false)
        })}
      />
    </>
  )
}

export function DialogTitle(props: HTMLAttributes<HTMLHeadingElement>) {
  return <h2 {...props} />
}

export function DialogDescription(props: HTMLAttributes<HTMLParagraphElement>) {
  return <p {...props} />
}

/** The host settles through onOpenChange(false) first, then the callback. */
export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  description,
  details,
  confirmLabel = 'Continue',
  cancelLabel = 'Cancel',
  tone = 'default',
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
  tone?: 'default' | 'danger'
  onConfirm: () => void
  onCancel?: () => void
}) {
  const settle = (confirmed: boolean) => {
    onOpenChange(false)
    if (confirmed) onConfirm()
    else onCancel?.()
  }
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) settle(false)
      }}
    >
      <DialogContent role="alertdialog">
        <DialogTitle>{title}</DialogTitle>
        {description ? (
          <DialogDescription>{description}</DialogDescription>
        ) : null}
        {details?.length ? (
          <ul>
            {details.map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ul>
        ) : null}
        <button type="button" onClick={() => settle(false)}>
          {cancelLabel}
        </button>
        <button type="button" data-tone={tone} onClick={() => settle(true)}>
          {confirmLabel}
        </button>
      </DialogContent>
    </Dialog>
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
  trigger: RefObject<HTMLButtonElement | null>
  content: RefObject<HTMLDivElement | null>
}>({
  open: false,
  setOpen() {},
  trigger: { current: null },
  content: { current: null },
})

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
  const trigger = useRef<HTMLButtonElement>(null)
  const content = useRef<HTMLDivElement>(null)
  return (
    <MenuContext.Provider
      value={{
        open: open ?? internal,
        setOpen(next) {
          setInternal(next)
          onOpenChange?.(next)
        },
        trigger,
        content,
      }}
    >
      {children}
    </MenuContext.Provider>
  )
}

export function DropdownMenuTrigger({ onClick, ...props }: Trigger) {
  const menu = useContext(MenuContext)
  return (
    <Slot
      aria-haspopup="menu"
      aria-expanded={menu.open}
      data-state={menu.open ? 'open' : 'closed'}
      ref={menu.trigger}
      {...props}
      onClick={compose(onClick, () => menu.setOpen(!menu.open))}
    />
  )
}

/** Closes on Escape and on a pointer down outside the menu and its trigger. */
export function DropdownMenuContent({
  align: _align,
  side: _side,
  sideOffset: _sideOffset,
  alignOffset: _alignOffset,
  collisionPadding: _collisionPadding,
  loop: _loop,
  onKeyDown,
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
  useOutsidePointer(menu.open, [menu.content, menu.trigger], () =>
    menu.setOpen(false),
  )
  return (
    <div
      role="menu"
      ref={menu.content}
      hidden={!menu.open}
      {...props}
      onKeyDown={compose(onKeyDown, (key) => {
        if (key.key === 'Escape') menu.setOpen(false)
      })}
    />
  )
}

/** Selecting closes the menu unless onSelect prevents the default. */
export function DropdownMenuItem({
  disabled,
  onSelect,
  textValue: _textValue,
  asChild: _asChild,
  onClick,
  onKeyDown,
  ...props
}: Omit<Div, 'onSelect'> & {
  disabled?: boolean
  onSelect?(event: Event): void
  textValue?: string
  asChild?: boolean
}) {
  const menu = useContext(MenuContext)
  return (
    <div
      role="menuitem"
      tabIndex={disabled ? undefined : -1}
      aria-disabled={disabled || undefined}
      data-disabled={disabled ? '' : undefined}
      {...props}
      onClick={compose(onClick, () => {
        if (disabled) return
        const select = new Event('menu.itemSelect', { cancelable: true })
        onSelect?.(select)
        if (!select.defaultPrevented) menu.setOpen(false)
      })}
      onKeyDown={compose(onKeyDown, (key) => {
        if (disabled || (key.key !== 'Enter' && key.key !== ' ')) return
        key.preventDefault()
        key.currentTarget.click()
      })}
    />
  )
}

export function DropdownMenuSeparator(props: Div) {
  // biome-ignore lint/a11y/useSemanticElements lint/a11y/useFocusableInteractive lint/a11y/useAriaPropsForRole: Radix's separator markup
  return <div role="separator" aria-orientation="horizontal" {...props} />
}

export function DropdownMenuLabel(props: Div) {
  return <div {...props} />
}

export function DropdownMenuGroup(props: Div) {
  return <div {...props} />
}

const RadioContext = createContext<{
  value?: string
  onValueChange?(value: string): void
}>({})

export function DropdownMenuRadioGroup({
  value,
  onValueChange,
  ...props
}: Div & { value?: string; onValueChange?(value: string): void }) {
  return (
    <RadioContext.Provider value={{ value, onValueChange }}>
      {/* biome-ignore lint/a11y/useSemanticElements: Radix's radio group markup */}
      <div role="group" {...props} />
    </RadioContext.Provider>
  )
}

/** A menu item that checks itself when it is the group's value. */
export function DropdownMenuRadioItem({
  value,
  onSelect,
  ...props
}: Omit<Div, 'onSelect'> & {
  value: string
  disabled?: boolean
  onSelect?(event: Event): void
  textValue?: string
}) {
  const radio = useContext(RadioContext)
  return (
    <DropdownMenuItem
      role="menuitemradio"
      aria-checked={radio.value === value}
      {...props}
      onSelect={(event) => {
        onSelect?.(event)
        radio.onValueChange?.(value)
      }}
    />
  )
}

/** The host's model picker, reduced to a labelled select of its options. */
export function ModelPicker({
  value,
  options,
  onChange,
  disabled,
  loading,
  placeholder = 'Choose a model',
  className,
}: {
  value: string | null
  options: { id: string; label: string }[]
  thinkingLevel: string
  onChange(next: string): void
  onThinkingLevelChange(next: string): void
  disabled?: boolean
  loading?: boolean
  placeholder?: string
  className?: string
  [key: string]: unknown
}) {
  return (
    <select
      className={className}
      aria-label="Model"
      data-model-picker=""
      value={value ?? ''}
      disabled={disabled || loading}
      onChange={(event) => onChange(event.target.value)}
    >
      <option value="">{placeholder}</option>
      {options.map((option) => (
        <option key={option.id} value={option.id}>
          {option.label}
        </option>
      ))}
    </select>
  )
}

// The host's BottomSheet is Radix Dialog drawn from the bottom edge.
export const BottomSheet = Dialog
export const BottomSheetTrigger = DialogTrigger
export const BottomSheetClose = DialogClose
export const BottomSheetContent = DialogContent
export const BottomSheetTitle = DialogTitle
export const BottomSheetDescription = DialogDescription

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
