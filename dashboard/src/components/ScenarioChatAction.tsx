import { ChevronDown, MessageCircle } from 'lucide-react'
import {
  type CSSProperties,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from 'react'
import { createPortal } from 'react-dom'
import { buttonClassName } from '@/design-system'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import { titleCase } from '@/lib/execution-view'
import {
  loadScenarioChatTargets,
  type ScenarioChatTarget,
  scenarioChatTargets,
} from '@/lib/scenario-chat'
import { useScenarioChat } from '@/lib/scenario-chat-context'

type ScenarioChatActionProps = {
  scenarioId: string
  executionId?: string | null
  subjectId?: string | null
  runId?: string | null
  detail?: DashboardExecutionDetail | null
  label?: string
  compact?: boolean
  className?: string
}

type MenuPosition = CSSProperties & { width: number }

function shortId(value: string) {
  return value.length > 18 ? `${value.slice(0, 12)}…${value.slice(-4)}` : value
}

function targetLabel(target: ScenarioChatTarget) {
  return `Run ${shortId(target.runId)} · attempt ${target.attemptNumber}`
}

export function ScenarioChatAction({
  scenarioId,
  executionId,
  subjectId,
  runId,
  detail,
  label = 'Open chat',
  compact = false,
  className = '',
}: ScenarioChatActionProps) {
  const { openChat } = useScenarioChat()
  const triggerRef = useRef<HTMLButtonElement>(null)
  const menuRef = useRef<HTMLDivElement>(null)
  const menuId = useId()
  const resolvedExecutionId = detail?.id ?? executionId ?? null
  const sourceKey = `${resolvedExecutionId ?? ''}:${subjectId ?? ''}:${scenarioId}:${runId ?? ''}`
  const detailTargets = useMemo(
    () =>
      detail ? scenarioChatTargets(detail, scenarioId, subjectId, runId) : null,
    [detail, runId, scenarioId, subjectId],
  )
  const [loadedTargets, setLoadedTargets] = useState<
    ScenarioChatTarget[] | null
  >(null)
  const [loading, setLoading] = useState(false)
  const [unavailable, setUnavailable] = useState(false)
  const [menuOpen, setMenuOpen] = useState(false)
  const [menuPosition, setMenuPosition] = useState<MenuPosition | null>(null)
  const targets = detailTargets ?? loadedTargets

  useEffect(() => {
    void sourceKey
    setLoadedTargets(null)
    setUnavailable(false)
    setMenuOpen(false)
  }, [sourceKey])

  useEffect(() => {
    if (!menuOpen) return
    const close = (event: PointerEvent) => {
      const node = event.target as Node
      if (
        !triggerRef.current?.contains(node) &&
        !menuRef.current?.contains(node)
      ) {
        setMenuOpen(false)
      }
    }
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      setMenuOpen(false)
      triggerRef.current?.focus()
    }
    document.addEventListener('pointerdown', close)
    document.addEventListener('keydown', handleEscape)
    return () => {
      document.removeEventListener('pointerdown', close)
      document.removeEventListener('keydown', handleEscape)
    }
  }, [menuOpen])

  if (!openChat || !resolvedExecutionId) return null
  if (detailTargets?.length === 0) return null

  const positionMenu = (count: number) => {
    const rect = triggerRef.current?.getBoundingClientRect()
    if (!rect) return
    const width = Math.min(320, window.innerWidth - 32)
    const estimatedHeight = Math.min(360, count * 60 + 16)
    const below = rect.bottom + 8
    const top =
      below + estimatedHeight <= window.innerHeight - 16
        ? below
        : Math.max(16, rect.top - estimatedHeight - 8)
    setMenuPosition({
      position: 'fixed',
      top,
      left: Math.min(
        window.innerWidth - width - 16,
        Math.max(16, rect.right - width),
      ),
      width,
    })
  }

  const resolveTargets = async () => {
    if (targets) return targets
    setLoading(true)
    try {
      const next = await loadScenarioChatTargets({
        executionId: resolvedExecutionId,
        scenarioId,
        subjectId,
        runId,
      })
      setLoadedTargets(next)
      if (next.length === 0) setUnavailable(true)
      return next
    } catch {
      setUnavailable(true)
      return []
    } finally {
      setLoading(false)
    }
  }

  const activate = async () => {
    const next = await resolveTargets()
    if (next.length === 1) {
      openChat(next[0].sessionId)
      return
    }
    if (next.length > 1) {
      positionMenu(next.length)
      setMenuOpen((current) => !current)
    }
  }

  const multiple = (targets?.length ?? 0) > 1
  const buttonLabel = unavailable
    ? 'Chat unavailable'
    : loading
      ? 'Loading chat…'
      : multiple
        ? `Chats · ${targets?.length}`
        : label

  return (
    <span className={`relative inline-flex ${className}`}>
      <button
        ref={triggerRef}
        className={buttonClassName({
          variant: 'secondary',
          size: compact ? 'compact' : 'default',
        })}
        type="button"
        title={compact ? buttonLabel : undefined}
        aria-label={`${buttonLabel} for ${titleCase(scenarioId)}`}
        aria-haspopup={multiple ? 'menu' : undefined}
        aria-expanded={multiple ? menuOpen : undefined}
        aria-controls={multiple && menuOpen ? menuId : undefined}
        disabled={loading || unavailable}
        onClick={(event) => {
          event.preventDefault()
          event.stopPropagation()
          void activate()
        }}
      >
        <MessageCircle size={15} aria-hidden="true" />
        {compact ? <span className="sr-only">{buttonLabel}</span> : buttonLabel}
        {!compact && multiple ? (
          <ChevronDown size={13} aria-hidden="true" />
        ) : null}
      </button>
      {menuOpen && menuPosition && targets && typeof document !== 'undefined'
        ? createPortal(
            <div
              ref={menuRef}
              id={menuId}
              className="z-[120] max-h-[min(360px,calc(100dvh-32px))] overflow-y-auto rounded-[var(--ds-radius-md)] border border-[var(--color-edge)] bg-panel p-1.5 shadow-panel"
              style={menuPosition}
              role="menu"
              aria-label={`Chat sessions for ${titleCase(scenarioId)}`}
            >
              {targets.map((target) => (
                <button
                  key={`${target.sessionId}:${target.attemptId}`}
                  className="flex min-h-12 w-full items-center justify-between gap-3 rounded-[var(--ds-radius-sm)] border-0 bg-transparent px-3 py-2 text-left hover:bg-panel-raised focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    setMenuOpen(false)
                    openChat(target.sessionId)
                  }}
                >
                  <span className="min-w-0">
                    <strong className="block truncate text-xs text-ink">
                      {targetLabel(target)}
                    </strong>
                    <span className="mt-1 block truncate font-mono text-[0.6875rem] text-ink-muted">
                      {target.current ? 'Current attempt' : 'Retry history'} ·{' '}
                      {shortId(target.sessionId)}
                    </span>
                  </span>
                  <span className="shrink-0 font-mono text-[0.6875rem] uppercase text-ink-muted">
                    {target.status ?? 'retained'}
                  </span>
                </button>
              ))}
            </div>,
            document.body,
          )
        : null}
    </span>
  )
}
