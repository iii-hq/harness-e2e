import { Check, ChevronDown } from 'lucide-react'
import {
  type KeyboardEvent,
  type ReactNode,
  useEffect,
  useId,
  useRef,
  useState,
} from 'react'

export type PickerOption = {
  value: string
  label: string
  /** A second line under the label (what a stack declares). */
  sub?: string
  /** Mono detail on the right: a count, a digest, warnings. */
  meta?: string
  metaTone?: 'faint' | 'warn'
}

export type PickerGroup = { label: string | null; options: PickerOption[] }

/** A trigger that opens a grouped listbox under it. Arrow keys move through
 *  the options, Enter or a click picks one, Escape closes and returns focus
 *  to the trigger without closing the dialog around it. */
export function Picker({
  id,
  label,
  groups,
  value,
  valueLabel,
  valueMeta,
  valueMetaTone = 'faint',
  placeholder = false,
  disabled = false,
  onPick,
  describedBy,
  children,
}: {
  id: string
  /** The listbox's accessible name. */
  label: string
  groups: PickerGroup[]
  value: string
  valueLabel: string
  valueMeta?: string
  valueMetaTone?: 'faint' | 'warn'
  /** The value reads as a prompt (nothing chosen yet). */
  placeholder?: boolean
  disabled?: boolean
  onPick: (value: string) => void
  describedBy?: string
  children?: ReactNode
}) {
  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const listId = `${useId()}-list`

  useEffect(() => {
    if (disabled) setOpen(false)
  }, [disabled])

  useEffect(() => {
    if (!open) return
    const outside = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false)
    }
    document.addEventListener('pointerdown', outside)
    const list = listRef.current
    ;(
      list?.querySelector<HTMLElement>('[aria-selected="true"]') ??
      list?.querySelector<HTMLElement>('[role="option"]')
    )?.focus()
    return () => document.removeEventListener('pointerdown', outside)
  }, [open])

  const close = () => {
    setOpen(false)
    triggerRef.current?.focus()
  }

  const onListKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const options = [
      ...(listRef.current?.querySelectorAll<HTMLElement>('[role="option"]') ??
        []),
    ]
    const index = options.indexOf(document.activeElement as HTMLElement)
    const move = (next: number) => {
      event.preventDefault()
      options[(next + options.length) % options.length]?.focus()
    }
    if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      close()
    } else if (event.key === 'ArrowDown') move(index + 1)
    else if (event.key === 'ArrowUp') move(index - 1)
    else if (event.key === 'Home') move(0)
    else if (event.key === 'End') move(options.length - 1)
    else if (event.key === 'Tab') setOpen(false)
  }

  return (
    <div className="rd-picker" ref={rootRef}>
      <button
        id={id}
        ref={triggerRef}
        type="button"
        className="rd-control rd-trigger"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        aria-describedby={describedBy}
        data-open={open || undefined}
        disabled={disabled}
        onClick={() => setOpen((current) => !current)}
        onKeyDown={(event) => {
          if (event.key === 'ArrowDown' && !open) {
            event.preventDefault()
            setOpen(true)
          }
        }}
      >
        <span
          className="rd-trigger-value"
          data-placeholder={placeholder || undefined}
          title={valueLabel}
        >
          {valueLabel}
        </span>
        {valueMeta ? (
          <span className="rd-meta" data-tone={valueMetaTone}>
            {valueMeta}
          </span>
        ) : null}
        <ChevronDown size={16} aria-hidden="true" className="rd-faint" />
      </button>
      {children}
      {open ? (
        <div
          id={listId}
          ref={listRef}
          role="listbox"
          aria-label={label}
          className="rd-popover"
          tabIndex={-1}
          onKeyDown={onListKeyDown}
        >
          {groups.map((group, groupIndex) =>
            group.options.length > 0 ? (
              // biome-ignore lint/a11y/useSemanticElements: a listbox group, not a fieldset
              <div
                key={group.label ?? `group-${groupIndex}`}
                role="group"
                aria-label={group.label ?? undefined}
              >
                {group.label ? (
                  <div className="rd-popover-label" aria-hidden="true">
                    {group.label}
                  </div>
                ) : null}
                {group.options.map((option) => {
                  const selected = option.value === value
                  return (
                    <button
                      key={option.value}
                      type="button"
                      role="option"
                      aria-selected={selected}
                      className="rd-option"
                      data-sub={option.sub ? true : undefined}
                      onClick={() => {
                        onPick(option.value)
                        close()
                      }}
                    >
                      <span className="rd-option-check" aria-hidden="true">
                        {selected ? <Check size={16} /> : null}
                      </span>
                      <span className="rd-option-text">
                        <span className="rd-ellipsis" title={option.label}>
                          {option.label}
                        </span>
                        {option.sub ? (
                          <span className="rd-option-sub rd-ellipsis">
                            {option.sub}
                          </span>
                        ) : null}
                      </span>
                      {option.meta ? (
                        <span
                          className="rd-meta"
                          data-tone={option.metaTone ?? 'faint'}
                        >
                          {option.meta}
                        </span>
                      ) : null}
                    </button>
                  )
                })}
              </div>
            ) : null,
          )}
        </div>
      ) : null}
    </div>
  )
}
