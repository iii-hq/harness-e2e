import { Check, ChevronDown, ChevronRight } from 'lucide-react'
import { useEffect, useId, useMemo, useRef, useState } from 'react'
import '@/design-system/styles.css'

export type ProviderModelOption = {
  label: string
  value: string
}

export type ProviderModelGroup = {
  provider: string
  models: Array<string | ProviderModelOption>
}

type ProviderModelDropdownProps = {
  groups: ProviderModelGroup[]
  value: string
  onChange: (value: string) => void
  placeholder: string
  ariaLabel: string
  disabled?: boolean
  required?: boolean
  optionValue?: (provider: string, model: string) => string
}

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

export function ProviderModelDropdown({
  groups,
  value,
  onChange,
  placeholder,
  ariaLabel,
  disabled = false,
  required = false,
  optionValue,
}: ProviderModelDropdownProps) {
  const rootRef = useRef<HTMLDivElement>(null)
  const [open, setOpen] = useState(false)
  const [expandedProviders, setExpandedProviders] = useState<Set<string>>(
    () => new Set(),
  )
  const instanceId = useId()
  const menuId = ['provider-model-menu', instanceId].join('-')
  const normalizedGroups = useMemo(
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

  const toggleProvider = (provider: string) => {
    setExpandedProviders((current) => {
      const next = new Set(current)
      if (next.has(provider)) next.delete(provider)
      else next.add(provider)
      return next
    })
  }

  return (
    <div className="relative min-w-0 w-full" ref={rootRef}>
      <button
        type="button"
        className="flex min-h-11 w-full items-center justify-between gap-2.5 rounded-lg border border-[var(--color-rule)] bg-[var(--color-panel-raised)] px-3 py-2 text-left text-sm text-[var(--color-ink)] transition-colors duration-[var(--ds-duration-fast)] hover:border-[var(--color-edge)] focus-visible:border-[var(--color-rule-focus)] focus-visible:[outline:2px_solid_var(--color-rule-focus)] focus-visible:[outline-offset:3px] disabled:cursor-not-allowed disabled:opacity-50 aria-expanded:border-[var(--color-edge)]"
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-controls={menuId}
        aria-expanded={open}
        data-required={required || undefined}
        disabled={disabled}
        onClick={() => setOpen((current) => !current)}
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
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
              <small className="overflow-hidden text-ellipsis whitespace-nowrap text-[0.64rem] font-medium text-[var(--color-ink-ghost)]">
                {selected.provider}
              </small>
            </>
          ) : (
            <span className="overflow-hidden text-ellipsis whitespace-nowrap text-xs text-[var(--color-ink-ghost)]">
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
        <div
          className="absolute z-50 mt-2 grid max-h-80 w-full min-w-[15rem] overflow-auto rounded-lg border border-[var(--color-edge)] bg-[var(--color-panel)] p-1.5 shadow-[var(--shadow-panel)]"
          id={menuId}
          role="listbox"
          aria-label={ariaLabel}
        >
          {normalizedGroups.length === 0 ? (
            <div className="p-5 text-center text-xs text-[var(--color-ink-ghost)]">
              No models available
            </div>
          ) : (
            normalizedGroups.map((group) => {
              const collapsed = !expandedProviders.has(group.provider)
              const groupId = [
                menuId,
                group.provider.replace(/[^a-zA-Z0-9_-]/g, '-'),
              ].join('-')
              return (
                <section
                  className="border-b border-[var(--color-rule)] py-1 last:border-b-0"
                  key={group.provider}
                >
                  <button
                    type="button"
                    className="flex min-h-9 w-full items-center gap-2 rounded-md border-0 bg-transparent px-2 text-left font-mono text-[0.68rem] font-semibold text-[var(--color-ink-faint)] transition-colors duration-[var(--ds-duration-fast)] hover:bg-[var(--color-panel-raised)] hover:text-[var(--color-ink)]"
                    aria-expanded={!collapsed}
                    aria-controls={groupId}
                    onClick={() => toggleProvider(group.provider)}
                  >
                    <ChevronRight
                      size={14}
                      aria-hidden="true"
                      className={`shrink-0 transition-transform duration-[var(--ds-duration-fast)] ${collapsed ? '' : 'rotate-90'}`}
                    />
                    <span className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap">
                      {group.provider}
                    </span>
                    <small className="font-mono text-[0.62rem] font-normal text-[var(--color-ink-ghost)]">
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
                          className={`flex min-h-9 w-full items-center justify-between gap-2 rounded-md border-0 px-2.5 text-left text-xs transition-colors duration-[var(--ds-duration-fast)] ${
                            model.value === value
                              ? 'bg-[var(--color-surface-hover)] font-semibold text-[var(--color-ink)]'
                              : 'bg-transparent text-[var(--color-ink-faint)] hover:bg-[var(--color-panel-raised)] hover:text-[var(--color-ink)]'
                          }`}
                          key={model.value}
                          onClick={() => {
                            onChange(model.value)
                            setOpen(false)
                          }}
                        >
                          <span>{model.label}</span>
                          {model.value === value && (
                            <Check size={14} aria-hidden="true" />
                          )}
                        </button>
                      ))}
                    </div>
                  )}
                </section>
              )
            })
          )}
        </div>
      )}
    </div>
  )
}
