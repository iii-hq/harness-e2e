declare module '@iii-dev/console-ui' {
  import type * as React from 'react'

  export interface ExtensionIii {
    browserId: string
    trigger<T = unknown>(
      functionId: string,
      payload?: Record<string, unknown>,
      options?: { timeoutMs?: number },
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
  }
  export const PageHeader: React.ComponentType<PageHeaderProps>

  export interface PageBodyProps extends React.HTMLAttributes<HTMLDivElement> {
    side?: 'left' | 'right'
  }
  export const PageBody: React.ComponentType<PageBodyProps>
  export const PageMain: React.ComponentType<React.HTMLAttributes<HTMLElement>>
}
