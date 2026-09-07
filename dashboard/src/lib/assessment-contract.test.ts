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
    // The run publishes one status: no advisory verdict, no effective status.
    expect(contract.runs[0]).not.toHaveProperty('ai_final_assessment')
    expect(contract.runs[0]).not.toHaveProperty('effective_status')
    // Assets are flat validation results, with no qualitative wrapper.
    expect(contract.runs[0]?.assets?.[0]?.asset_id).toBe('result')
    expect(contract.runs[0]?.assets?.[0]?.outcome).toBe('valid')
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
