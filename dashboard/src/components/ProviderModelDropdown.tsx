import { Check, ChevronDown, ChevronRight } from 'lucide-react'
import {
  type KeyboardEvent as ReactKeyboardEvent,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from 'react'
import '@/design-system/styles.css'

export type ProviderModelOption = {
  label: string
  value: string
}

export type ProviderModelGroup = {
  provider: string
  models: Array<string | ProviderModelOption>
}

type NormalizedGroup = {
  provider: string
  models: ProviderModelOption[]
}

type ProviderModelDropdownProps = {
  groups: ProviderModelGroup[]
  value: string
  onChange: (value: string) => void
  placeholder: string
  ariaLabel: string
  /** Id of a visible label. When given it labels the control instead of ariaLabel. */
  labelledBy?: string
  /** Renders a first option that clears the value, e.g. "Default judge". */
  clearLabel?: string
  disabled?: boolean
  required?: boolean
  optionValue?: (provider: string, model: string) => string
}

const OPTION_SELECTOR = '[role="option"]'

function normalizeOption(
  provider: string,
  model: string | ProviderModelOption,
  optionValue?: (provider: string, model: string) => string,
): ProviderModelOption {
  if (typeof model !== 'string') return model
  return {
    label: model,
    value: optionValue?.(provider, model) ?? [provider, model].join('\n'),
  }
}

function optionClassName(selected: boolean) {
  return `flex min-h-9 w-full items-center justify-between gap-2 rounded-[6px] border-0 px-2.5 text-left text-xs transition-colors duration-[var(--ds-duration-fast)] focus-visible:[outline:2px_solid_var(--color-rule-focus)] focus-visible:[outline-offset:-2px] ${
    selected
      ? 'bg-[var(--color-surface-hover)] font-semibold text-ink'
      : 'bg-transparent text-[var(--color-ink-faint)] hover:bg-panel-raised hover:text-ink'
  }`
}

export function ProviderModelDropdown({
  groups,
  value,
  onChange,
  placeholder,
  ariaLabel,
  labelledBy,
  clearLabel,
  disabled = false,
  required = false,
  optionValue,
}: ProviderModelDropdownProps) {
  const rootRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const menuRef = useRef<HTMLDivElement>(null)
  const [open, setOpen] = useState(false)
  // Audit PN-07 / RS-08: providers start expanded so a model is one click
  // away; collapsing a long provider is opt-in.
  const [collapsedProviders, setCollapsedProviders] = useState<Set<string>>(
    () => new Set(),
  )
  const instanceId = useId()
  const menuId = ['provider-model-menu', instanceId].join('-')
  const normalizedGroups = useMemo<NormalizedGroup[]>(
    () =>
      groups.map((group) => ({
        provider: group.provider,
        models: group.models.map((model) =>
          normalizeOption(group.provider, model, optionValue),
        ),
      })),
    [groups, optionValue],
  )
  const selected = normalizedGroups
    .flatMap((group) =>
      group.models.map((model) => ({ ...model, provider: group.provider })),
    )
    .find((model) => model.value === value)

  useEffect(() => {
    if (disabled) setOpen(false)
  }, [disabled])

  useEffect(() => {
    if (!open) return
    const closeOnOutsidePointer = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false)
    }
    document.addEventListener('pointerdown', closeOnOutsidePointer)
    return () =>
      document.removeEventListener('pointerdown', closeOnOutsidePointer)
  }, [open])

  // Focus enters the list when it opens, on the current choice when there
  // is one, so arrow keys work immediately.
  useEffect(() => {
    if (!open) return
    const menu = menuRef.current
    const target =
      menu?.querySelector<HTMLElement>(
        '[role="option"][aria-selected="true"]',
      ) ?? menu?.querySelector<HTMLElement>(OPTION_SELECTOR)
    target?.focus()
  }, [open])

  const close = (restoreFocus: boolean) => {
    setOpen(false)
    if (restoreFocus) triggerRef.current?.focus()
  }

  const select = (next: string) => {
    onChange(next)
    close(true)
  }

  const moveFocus = (direction: 1 | -1) => {
    const options = [
      ...(menuRef.current?.querySelectorAll<HTMLElement>(OPTION_SELECTOR) ??
        []),
    ]
    if (options.length === 0) return
    const index = options.indexOf(document.activeElement as HTMLElement)
    const next =
      index === -1
        ? direction === 1
          ? 0
          : options.length - 1
        : (index + direction + options.length) % options.length
    options[next]?.focus()
  }

  const handleMenuKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Escape') {
      // preventDefault also keeps a surrounding <dialog> from closing.
      event.preventDefault()
      close(true)
    } else if (event.key === 'ArrowDown') {
      event.preventDefault()
      moveFocus(1)
    } else if (event.key === 'ArrowUp') {
      event.preventDefault()
      moveFocus(-1)
    } else if (event.key === 'Tab') {
      setOpen(false)
    }
  }

  const toggleProvider = (provider: string) => {
    setCollapsedProviders((current) => {
      const next = new Set(current)
      if (next.has(provider)) next.delete(provider)
      else next.add(provider)
      return next
    })
  }

  return (
    <div className="relative min-w-0 w-full" ref={rootRef}>
      <button
        ref={triggerRef}
        type="button"
        className="flex min-h-11 w-full items-center justify-between gap-2.5 rounded-[6px] border border-[var(--color-rule)] bg-panel-raised px-3 py-2 text-left text-sm text-ink transition-colors duration-[var(--ds-duration-fast)] hover:border-[var(--color-edge)] focus-visible:border-[var(--color-rule-focus)] focus-visible:[outline:2px_solid_var(--color-rule-focus)] focus-visible:[outline-offset:3px] disabled:cursor-not-allowed disabled:opacity-50 aria-expanded:border-[var(--color-edge)]"
        aria-label={labelledBy ? undefined : ariaLabel}
        aria-labelledby={labelledBy}
        aria-haspopup="listbox"
        aria-controls={menuId}
        aria-expanded={open}
        data-required={required || undefined}
        disabled={disabled}
        onClick={() => setOpen((current) => !current)}
        onKeyDown={(event) => {
          if (event.key === 'Escape' && open) {
            event.preventDefault()
            setOpen(false)
            return
          }
          if (
            (event.key === 'Enter' ||
              event.key === ' ' ||
              event.key === 'ArrowDown') &&
            !open
          ) {
            event.preventDefault()
            setOpen(true)
          }
        }}
      >
        <span className="flex min-w-0 flex-1 flex-col gap-0.5 overflow-hidden">
          {selected ? (
            <>
              <strong className="overflow-hidden text-ellipsis whitespace-nowrap text-xs font-semibold">
                {selected.label}
              </strong>
              <small className="overflow-hidden text-ellipsis whitespace-nowrap text-[0.6875rem] font-medium text-ink-muted">
                {selected.provider}
              </small>
            </>
          ) : (
            <span className="overflow-hidden text-ellipsis whitespace-nowrap text-xs text-ink-muted">
              {placeholder}
            </span>
          )}
        </span>
        <ChevronDown
          size={15}
          aria-hidden="true"
          className={`shrink-0 transition-transform duration-[var(--ds-duration-fast)] ${open ? 'rotate-180' : ''}`}
        />
      </button>

      {open && (
        <ProviderModelMenu
          ref={menuRef}
          id={menuId}
          ariaLabel={labelledBy ? undefined : ariaLabel}
          labelledBy={labelledBy}
          groups={normalizedGroups}
          value={value}
          clearLabel={clearLabel}
          collapsedProviders={collapsedProviders}
          onToggleProvider={toggleProvider}
          onSelect={select}
          onKeyDown={handleMenuKeyDown}
        />
      )}
    </div>
  )
}

/** The open list. Exported so its markup can be rendered without a click. */
export function ProviderModelMenu({
  ref,
  id,
  ariaLabel,
  labelledBy,
  groups,
  value,
  clearLabel,
  collapsedProviders,
  onToggleProvider,
  onSelect,
  onKeyDown,
}: {
  ref?: React.Ref<HTMLDivElement>
  id: string
  ariaLabel?: string
  labelledBy?: string
  groups: NormalizedGroup[]
  value: string
  clearLabel?: string
  collapsedProviders: Set<string>
  onToggleProvider: (provider: string) => void
  onSelect: (value: string) => void
  onKeyDown?: (event: ReactKeyboardEvent<HTMLDivElement>) => void
}) {
  return (
    <div
      ref={ref}
      onKeyDown={onKeyDown}
      className="absolute z-50 mt-2 grid max-h-80 w-full min-w-[15rem] overflow-auto rounded-[6px] border border-[var(--color-edge)] bg-panel p-1.5 shadow-[var(--shadow-panel)]"
      id={id}
      role="listbox"
      aria-label={ariaLabel}
      aria-labelledby={labelledBy}
    >
      {clearLabel ? (
        <button
          type="button"
          role="option"
          aria-selected={value === ''}
          className={`${optionClassName(value === '')} mb-1`}
          onClick={() => onSelect('')}
        >
          <span>{clearLabel}</span>
          {value === '' && <Check size={14} aria-hidden="true" />}
        </button>
      ) : null}
      {groups.length === 0 ? (
        <div className="p-5 text-center text-xs text-ink-muted">
          No models available
        </div>
      ) : (
        groups.map((group) => {
          const collapsed = collapsedProviders.has(group.provider)
          const groupId = [
            id,
            group.provider.replace(/[^a-zA-Z0-9_-]/g, '-'),
          ].join('-')
          const groupLabelId = `${groupId}-label`
          return (
            // biome-ignore lint/a11y/useSemanticElements: a fieldset is not a valid child of a listbox
            <div
              className="border-b border-[var(--color-rule)] py-1 last:border-b-0"
              role="group"
              aria-labelledby={groupLabelId}
              key={group.provider}
            >
              <button
                type="button"
                id={groupLabelId}
                tabIndex={-1}
                className="flex min-h-9 w-full items-center gap-2 rounded-[6px] border-0 bg-transparent px-2 text-left font-mono text-[0.6875rem] font-semibold text-[var(--color-ink-faint)] transition-colors duration-[var(--ds-duration-fast)] hover:bg-panel-raised hover:text-ink"
                aria-expanded={!collapsed}
                aria-controls={groupId}
                onClick={() => onToggleProvider(group.provider)}
              >
                <ChevronRight
                  size={14}
                  aria-hidden="true"
                  className={`shrink-0 transition-transform duration-[var(--ds-duration-fast)] ${collapsed ? '' : 'rotate-90'}`}
                />
                <span className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap">
                  {group.provider}
                </span>
                <small className="font-mono text-[0.6875rem] font-normal text-ink-muted">
                  {group.models.length}
                </small>
              </button>
              {!collapsed && (
                <div className="grid gap-0.5 pb-1 pl-5" id={groupId}>
                  {group.models.map((model) => (
                    <button
                      type="button"
                      role="option"
                      aria-selected={model.value === value}
                      className={optionClassName(model.value === value)}
                      key={model.value}
                      onClick={() => onSelect(model.value)}
                    >
                      <span>{model.label}</span>
                      {model.value === value && (
                        <Check size={14} aria-hidden="true" />
                      )}
                    </button>
                  ))}
                </div>
              )}
            </div>
          )
        })
      )}
    </div>
  )
}
