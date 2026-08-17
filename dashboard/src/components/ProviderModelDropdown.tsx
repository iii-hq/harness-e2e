import { Check, ChevronDown, ChevronRight } from 'lucide-react'
import { useEffect, useId, useMemo, useRef, useState } from 'react'

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
    <div className="provider-model-dropdown" ref={rootRef}>
      <button
        type="button"
        className="provider-model-trigger"
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
        <span className="provider-model-selection">
          {selected ? (
            <>
              <strong>{selected.label}</strong>
              <small>{selected.provider}</small>
            </>
          ) : (
            <span>{placeholder}</span>
          )}
        </span>
        <ChevronDown
          size={15}
          aria-hidden="true"
          className={
            open ? 'provider-model-chevron is-open' : 'provider-model-chevron'
          }
        />
      </button>

      {open && (
        <div
          className="provider-model-menu"
          id={menuId}
          role="listbox"
          aria-label={ariaLabel}
        >
          {normalizedGroups.length === 0 ? (
            <div className="provider-model-empty">No models available</div>
          ) : (
            normalizedGroups.map((group) => {
              const collapsed = !expandedProviders.has(group.provider)
              const groupId = [
                menuId,
                group.provider.replace(/[^a-zA-Z0-9_-]/g, '-'),
              ].join('-')
              return (
                <section
                  className={
                    collapsed
                      ? 'provider-model-group is-collapsed'
                      : 'provider-model-group'
                  }
                  key={group.provider}
                >
                  <button
                    type="button"
                    className="provider-model-group-toggle"
                    aria-expanded={!collapsed}
                    aria-controls={groupId}
                    onClick={() => toggleProvider(group.provider)}
                  >
                    <ChevronRight
                      size={14}
                      aria-hidden="true"
                      className="provider-model-group-chevron"
                    />
                    <span>{group.provider}</span>
                    <small>{group.models.length}</small>
                  </button>
                  {!collapsed && (
                    <div className="provider-model-options" id={groupId}>
                      {group.models.map((model) => (
                        <button
                          type="button"
                          role="option"
                          aria-selected={model.value === value}
                          className={
                            model.value === value
                              ? 'provider-model-option is-selected'
                              : 'provider-model-option'
                          }
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
