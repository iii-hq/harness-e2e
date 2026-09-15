import type { Host } from '@iii-dev/console-ui'
import { Tooltip, TooltipContent, TooltipTrigger } from '@iii-dev/console-ui'
import { Search } from 'lucide-react'
import { createContext, useContext } from 'react'
import { Button } from '@/design-system'
import { investigationPrompt } from '@/lib/investigation'

export const InvestigationContext =
  createContext<NonNullable<Host['chat']>['openDraft']>(undefined)

export function InvestigationAction(
  context: Parameters<typeof investigationPrompt>[0],
) {
  const openDraft = useContext(InvestigationContext)
  if (!openDraft) return null
  const comparing = Boolean(context.comparisonExecutionId)
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="secondary"
          onClick={() =>
            openDraft({
              title: comparing
                ? 'E2E comparison investigation'
                : 'E2E execution investigation',
              text: investigationPrompt(context),
            })
          }
        >
          <Search size={16} aria-hidden="true" />
          {comparing ? 'Investigate comparison' : 'Investigate execution'}
        </Button>
      </TooltipTrigger>
      <TooltipContent>
        Open a new Harness chat with an editable investigation prompt
      </TooltipContent>
    </Tooltip>
  )
}
