import { describe, expect, it } from 'vitest'
import { credentialStatus } from '@/components/ProviderCredentials'

describe('provider credentials', () => {
  it('names where a credential is set from, never its value', () => {
    const credential = { name: 'OPENAI_API_KEY', providers: ['openai'] }
    expect(credentialStatus({ ...credential, set: false })).toBe('Not set')
    expect(
      credentialStatus({ ...credential, set: true, source: 'console' }),
    ).toBe('Set here')
    expect(
      credentialStatus({
        ...credential,
        set: true,
        source: 'provider_env_file',
      }),
    ).toBe('Set by the worker’s provider_env_file')
    expect(
      credentialStatus({ ...credential, set: true, source: 'environment' }),
    ).toBe('Set by the worker’s environment')
  })
})
