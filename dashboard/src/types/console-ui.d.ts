declare module '@iii-dev/console-ui' {
  import type * as React from 'react'

  export interface ExtensionIii {
    browserId: string
    trigger<T = unknown>(
      functionId: string,
      payload?: Record<string, unknown>,
      options?: { timeoutMs?: number; namespace?: string },
    ): Promise<T>
    on<P = unknown>(
      functionId: string,
      handler: (payload: P) => void | Promise<void>,
    ): () => void
    registerTrigger(input: {
      type: string
      function_id: string
      config: Record<string, unknown>
    }): () => void
  }

  export interface PageRenderProps {
    panelSide: 'left' | 'right'
    tabId: string
    onRequestClose?: () => void
    workingDir?: string | null
  }

  export interface Host {
    iii: ExtensionIii
    useTheme(): 'light' | 'dark'
    pages: {
      register(page: {
        id: string
        title: string
        render: React.ComponentType<PageRenderProps>
      }): () => void
    }
  }

  export interface PageShellProps
    extends React.HTMLAttributes<HTMLDivElement> {}
  export const PageShell: React.ComponentType<PageShellProps>

  export interface PageHeaderProps {
    icon?: React.ReactNode
    title?: React.ReactNode
    description?: React.ReactNode
    actions?: React.ReactNode
    onClose?: () => void
    className?: string
    children?: React.ReactNode
  }
  export const PageHeader: React.ComponentType<PageHeaderProps>

  export interface PageBodyProps extends React.HTMLAttributes<HTMLDivElement> {
    side?: 'left' | 'right'
  }
  export const PageBody: React.ComponentType<PageBodyProps>
  export const PageMain: React.ComponentType<React.HTMLAttributes<HTMLElement>>

  export interface ButtonProps
    extends React.ButtonHTMLAttributes<HTMLButtonElement> {
    variant?: 'primary' | 'ghost' | 'pill' | 'icon' | 'terminal' | 'wiggle'
    size?: 'sm' | 'md' | 'lg' | 'icon'
    asChild?: boolean
  }
  export const Button: React.ComponentType<ButtonProps>

  export interface InputProps
    extends Omit<
      React.InputHTMLAttributes<HTMLInputElement>,
      'onChange' | 'value'
    > {
    value: string
    onChange: (next: string) => void
    preserveCase?: boolean
  }
  export const Input: React.ComponentType<InputProps>

  export interface SelectOption<T extends string = string> {
    value: T
    label: string
    title?: string
  }
  export interface SelectProps<T extends string = string> {
    value: T | undefined
    options?: SelectOption<T>[]
    onChange: (next: T) => void
    disabled?: boolean
    className?: string
    'aria-label'?: string
    placeholder?: string
  }
  export const Select: <T extends string = string>(
    props: SelectProps<T>,
  ) => React.ReactNode

  export interface TabsProps extends React.HTMLAttributes<HTMLDivElement> {
    value?: string
    defaultValue?: string
    onValueChange?(value: string): void
    orientation?: 'horizontal' | 'vertical'
    dir?: 'ltr' | 'rtl'
  }
  export const Tabs: React.ComponentType<TabsProps>
  export const TabsList: React.ComponentType<
    React.HTMLAttributes<HTMLDivElement>
  >
  export interface TabsTriggerProps
    extends React.ButtonHTMLAttributes<HTMLButtonElement> {
    value: string
    /** Semantic 16px glyph; the host infers one from `value`, `false` hides it. */
    icon?: React.ReactNode | false
  }
  export const TabsTrigger: React.ComponentType<TabsTriggerProps>

  export const Skeleton: React.ComponentType<
    React.HTMLAttributes<HTMLSpanElement>
  >

  export type TableDensity = 'comfortable' | 'compact'
  export interface TableProps
    extends React.TableHTMLAttributes<HTMLTableElement> {
    density?: TableDensity
  }
  export const TableViewport: React.ComponentType<
    React.HTMLAttributes<HTMLDivElement>
  >
  export const TableFrame: React.ComponentType<
    React.HTMLAttributes<HTMLDivElement>
  >
  export const Table: React.ComponentType<TableProps>
  export const TableHeader: React.ComponentType<
    React.HTMLAttributes<HTMLTableSectionElement>
  >
  export const TableBody: React.ComponentType<
    React.HTMLAttributes<HTMLTableSectionElement>
  >
  export const TableFooter: React.ComponentType<
    React.HTMLAttributes<HTMLTableSectionElement>
  >
  export interface TableRowProps
    extends React.HTMLAttributes<HTMLTableRowElement> {
    interactive?: boolean
    selected?: boolean
  }
  export const TableRow: React.ComponentType<TableRowProps>
  export const TableHead: React.ComponentType<
    React.ThHTMLAttributes<HTMLTableCellElement>
  >
  export const TableCell: React.ComponentType<
    React.TdHTMLAttributes<HTMLTableCellElement>
  >
  export const TableCaption: React.ComponentType<
    React.HTMLAttributes<HTMLTableCaptionElement>
  >

  export type ChipTone = 'neutral' | 'accent' | 'success' | 'warning' | 'danger'
  export interface ChipProps extends React.HTMLAttributes<HTMLSpanElement> {
    tone?: ChipTone
    selected?: boolean
  }
  export const Chip: React.ComponentType<ChipProps>

  export type BadgeVariant = 'default' | 'ok' | 'warn' | 'alert' | 'accent'
  export interface BadgeProps extends React.HTMLAttributes<HTMLSpanElement> {
    variant?: BadgeVariant
  }
  export const Badge: React.ComponentType<BadgeProps>

  export interface StatusDotProps
    extends React.HTMLAttributes<HTMLSpanElement> {
    tone?: 'accent' | 'alert' | 'warn' | 'ink' | 'ok'
    pulse?: boolean
  }
  export const StatusDot: React.ComponentType<StatusDotProps>
  export interface StatusPanelProps {
    variant?: 'info' | 'success' | 'warn' | 'alert'
    icon?: React.ReactNode
    headline: React.ReactNode
    detail?: React.ReactNode
    className?: string
  }
  export const StatusPanel: React.ComponentType<StatusPanelProps>
  export interface EmptyStateProps {
    icon?: React.ComponentType<{ className?: string }>
    title: string
    description: string
    action?: { label: string; onClick: () => void }
  }
  export const EmptyState: React.ComponentType<EmptyStateProps>
  export const Dialog: React.ComponentType<{
    open?: boolean
    onOpenChange?(open: boolean): void
    children?: React.ReactNode
  }>
  export const DropdownMenu: React.ComponentType<{ children?: React.ReactNode }>
  export const PageSidebar: React.ComponentType<
    React.HTMLAttributes<HTMLElement> & { width?: number }
  >
}
