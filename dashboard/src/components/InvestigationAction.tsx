import type { Host } from '@iii-dev/console-ui'
import { Tooltip, TooltipContent, TooltipTrigger } from '@iii-dev/console-ui'
import { Search } from 'lucide-react'
import { createContext, useContext } from 'react'
import { buttonClassName } from '@/design-system'
import { type Investigation, investigationPrompt } from '@/lib/investigation'
import { UNKNOWN_SCENARIO } from '@/lib/scenario-matrix'

export const InvestigationContext =
  createContext<NonNullable<Host['chat']>['openDraft']>(undefined)

/** Opens the chat on an investigation; null when the host has no chat. For
 *  a menu item, which takes a callback, not a button. */
export function useInvestigation() {
  const openDraft = useContext(InvestigationContext)
  if (!openDraft) return null
  return (asked: Investigation) => {
    // A report that could not be read has no test to start from.
    const context =
      asked.focus?.scenarioId === UNKNOWN_SCENARIO
        ? { ...asked, focus: undefined }
        : asked
    const focus = context.focus?.scenarioId
    openDraft({
      title: context.comparisonExecutionId
        ? 'E2E comparison investigation'
        : focus
          ? `E2E investigation · ${focus}`
          : 'E2E execution investigation',
      text: investigationPrompt(context),
    })
  }
}

export function InvestigationAction({
  label,
  buttonClass,
  ...context
}: Investigation & {
  label?: string
  /** Replaces the button's own class (a row or page with its own style). */
  buttonClass?: string
}) {
  const investigate = useInvestigation()
  if (!investigate) return null
  const focus = context.focus?.scenarioId
  const text =
    label ??
    (context.comparisonExecutionId
      ? 'Investigate comparison'
      : 'Investigate execution')
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          className={buttonClass ?? buttonClassName({ variant: 'secondary' })}
          // Named after its test, beginning with the words it shows.
          aria-label={
            focus
              ? text === 'Investigate'
                ? `Investigate ${focus}`
                : `${text} · ${focus}`
              : undefined
          }
          onClick={() => investigate(context)}
        >
          <Search size={buttonClass ? 14 : 16} aria-hidden="true" />
          {text}
        </button>
      </TooltipTrigger>
      <TooltipContent>
        Open a new Harness chat with an editable investigation prompt
      </TooltipContent>
    </Tooltip>
  )
}
