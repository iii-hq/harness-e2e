import type { Host } from '@iii-dev/console-ui'
import { Tooltip, TooltipContent, TooltipTrigger } from '@iii-dev/console-ui'
import { Search } from 'lucide-react'
import { createContext, useContext } from 'react'
import { buttonClassName } from '@/design-system'
import { type Investigation, investigationPrompt } from '@/lib/investigation'

export const InvestigationContext =
  createContext<NonNullable<Host['chat']>['openDraft']>(undefined)

export function InvestigationAction({
  label,
  buttonClass,
  ...context
}: Investigation & {
  label?: string
  /** Replaces the button's own class (a row or page with its own style). */
  buttonClass?: string
}) {
  const openDraft = useContext(InvestigationContext)
  if (!openDraft) return null
  const comparing = Boolean(context.comparisonExecutionId)
  const focus = context.focus?.scenarioId
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          className={buttonClass ?? buttonClassName({ variant: 'secondary' })}
          aria-label={focus ? `Investigate ${focus}` : undefined}
          onClick={() =>
            openDraft({
              title: comparing
                ? 'E2E comparison investigation'
                : focus
                  ? `E2E investigation · ${focus}`
                  : 'E2E execution investigation',
              text: investigationPrompt(context),
            })
          }
        >
          <Search size={buttonClass ? 14 : 16} aria-hidden="true" />
          {label ??
            (comparing ? 'Investigate comparison' : 'Investigate execution')}
        </button>
      </TooltipTrigger>
      <TooltipContent>
        Open a new Harness chat with an editable investigation prompt
      </TooltipContent>
    </Tooltip>
  )
}
