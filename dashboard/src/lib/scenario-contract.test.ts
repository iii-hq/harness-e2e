import { describe, expect, it } from 'vitest'
import {
  contractFingerprint,
  scenarioContractFingerprint,
} from '@/lib/scenario-contract'

describe('scenario contract fingerprint', () => {
  // Pinned against `contract_fingerprint_matches_the_browser_implementation`
  // in src/dashboard.rs. The hashed object carries no definition digest.
  it('matches the harness implementation', () => {
    expect(
      contractFingerprint({
        case_id: 'direct_answer:canonical',
        execution_policy: {},
        scenario_id: 'direct_answer',
      }),
    ).toBe('fnv1a32:51327792')
  })

  it('hashes keys in a stable order whatever the insertion order', () => {
    expect(
      contractFingerprint({
        scenario_id: 'direct_answer',
        execution_policy: {},
        case_id: 'direct_answer:canonical',
      }),
    ).toBe('fnv1a32:51327792')
  })

  it('omits the case when a scenario never materialized one', () => {
    expect(
      scenarioContractFingerprint({
        scenario_id: 'direct_answer',
        case_id: 'direct_answer:canonical',
        execution_policy: {},
      }),
    ).toBe('fnv1a32:51327792')
    expect(
      scenarioContractFingerprint({
        scenario_id: 'direct_answer',
        case_id: 'direct_answer:canonical',
        case: { seed: 1 },
        execution_policy: {},
      }),
    ).not.toBe('fnv1a32:51327792')
  })
})
