import { describe, expect, it } from 'vitest'
import {
  credentialStatus,
  importSummary,
} from '@/components/ProviderCredentials'

describe('provider credentials', () => {
  it('says what an import found and what it did not', () => {
    expect(
      importSummary({
        found: ['DEEPSEEK_API_KEY'],
        not_found: ['OPENAI_API_KEY', 'ZAI_API_KEY'],
        credentials: [],
      }),
    ).toBe(
      "Set from the worker's environment: DEEPSEEK_API_KEY. Not found there: OPENAI_API_KEY, ZAI_API_KEY.",
    )
    expect(
      importSummary({
        found: [],
        not_found: ['OPENAI_API_KEY'],
        credentials: [],
      }),
    ).toBe(
      "None of the known keys is in the worker's environment (OPENAI_API_KEY).",
    )
  })

  it('names where a credential is set from, never its value', () => {
    const credential = { name: 'OPENAI_API_KEY', providers: ['openai'] }
    expect(credentialStatus({ ...credential, set: false })).toBe('not set')
    expect(
      credentialStatus({ ...credential, set: true, source: 'console' }),
    ).toBe('set here')
    expect(
      credentialStatus({
        ...credential,
        set: true,
        source: 'provider_env_file',
      }),
    ).toBe('set by the worker’s provider_env_file')
  })
})
