import { describe, expect, it } from 'vitest'
import {
  AssessmentContractError,
  readAssessmentContract,
  summarizeAssessmentContract,
} from '@/lib/assessment-contract'
import resultFixture from '../../../tests/fixtures/results/results-assessment-contract.json'

describe('assessment result contract', () => {
  it('preserves the shared current contract fixture', () => {
    const result = resultFixture as {
      assessment_contract: unknown
      dashboard_projection: { summary: unknown }
    }
    const contract = readAssessmentContract(result)

    expect(contract).toEqual(result.assessment_contract)
    expect(contract.runs[0]?.system_status).toBe('hard_gate_failed')
    expect(contract.runs[0]?.ai_final_assessment.availability).toBe(
      'unavailable',
    )
    expect(contract.runs[0]?.effective_status).toBe('hard_gate_failed')
    expect(summarizeAssessmentContract(contract)).toEqual(
      result.dashboard_projection.summary,
    )
  })

  it('rejects versioned result and assessment payloads', () => {
    expect(() => readAssessmentContract({ schema_version: 3 })).toThrow(
      AssessmentContractError,
    )
    expect(() =>
      readAssessmentContract({
        assessment_contract: { contract_version: 1, runs: [] },
      }),
    ).toThrow(AssessmentContractError)
  })

  it('rejects results without the assessment contract', () => {
    expect(() => readAssessmentContract({ scenarios: [] })).toThrow(
      AssessmentContractError,
    )
  })
})
