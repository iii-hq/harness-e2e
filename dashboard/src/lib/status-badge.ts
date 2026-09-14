import type { BadgeVariant } from '@iii-dev/console-ui'
import type { OperationalStatus } from '@/design-system/primitives'

/** The Console's badge tones for the dashboard's operational statuses. */
export function badgeVariantForStatus(
  status: OperationalStatus | string,
): BadgeVariant {
  switch (status) {
    case 'passed':
      return 'ok'
    case 'failed':
    case 'hard_gate_failed':
    case 'error':
      return 'alert'
    case 'inconclusive':
    case 'incomplete':
    case 'cancelling':
    case 'partial':
      return 'warn'
    case 'running':
      return 'accent'
    default:
      if (/error|fail/.test(status)) return 'alert'
      return 'default'
  }
}
