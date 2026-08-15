import { describe, expect, it } from 'vitest'
import {
  AssessmentContractError,
  normalizeAssessmentContract,
} from '@/lib/assessment-contract'
import legacyResult from '../../../tests/fixtures/results/results-v2-without-assessments.json'
import v3Result from '../../../tests/fixtures/results/results-v3-assessment-contract.json'

describe('assessment result contract', () => {
  it('preserves the shared v3 contract fixture', () => {
    const result = v3Result as {
      assessment_contract: unknown
    }
    const contract = normalizeAssessmentContract(result)

    expect(contract).toEqual(result.assessment_contract)
    expect(contract.runs[0]?.system_status).toBe('hard_gate_failed')
    expect(contract.runs[0]?.ai_final_assessment.availability).toBe(
      'unavailable',
    )
    expect(contract.runs[0]?.effective_status).toBe('hard_gate_failed')
  })

  it('normalizes legacy results to explicit unavailable states', () => {
    const contract = normalizeAssessmentContract(legacyResult)

    expect(contract.contract_version).toBe(1)
    expect(contract.runs[0]).toMatchObject({
      run_id: 'legacy-run',
      attempt_id: 'legacy-attempt',
      system_status: 'unavailable',
      effective_status: 'unavailable',
      ai_final_assessment: { availability: 'not_evaluated' },
    })
  })

  it('rejects v3 results without the contract', () => {
    expect(() =>
      normalizeAssessmentContract({ schema_version: 3, scenarios: [] }),
    ).toThrow(AssessmentContractError)
  })
})
