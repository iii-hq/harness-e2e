import { describe, expect, it } from 'vitest'
import {
  AssessmentContractError,
  readAssessmentContract,
  summarizeAssessmentContract,
} from '@/lib/assessment-contract'
import { RESULT_CONTRACT_SHA256 } from '@/lib/result-contract.generated'
import resultFixture from '../../../tests/fixtures/results/results-assessment-contract.json'

describe('assessment result contract', () => {
  it('preserves the shared current contract fixture', () => {
    const result = {
      result_contract_sha256: RESULT_CONTRACT_SHA256,
      ...(resultFixture as {
        assessment_contract: unknown
        dashboard_projection: { summary: unknown }
      }),
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

  it('rejects payloads without the results contract digest', () => {
    expect(() =>
      readAssessmentContract({ assessment_contract: { runs: [] } }),
    ).toThrow(AssessmentContractError)
    expect(() =>
      readAssessmentContract({
        result_contract_sha256: 'results-v4',
        assessment_contract: { runs: [] },
      }),
    ).toThrow(AssessmentContractError)
  })

  it('rejects versioned assessment contracts', () => {
    expect(() =>
      readAssessmentContract({
        result_contract_sha256: RESULT_CONTRACT_SHA256,
        assessment_contract: { contract_version: 1, runs: [] },
      }),
    ).toThrow(AssessmentContractError)
  })

  it('rejects results without the assessment contract', () => {
    expect(() =>
      readAssessmentContract({
        result_contract_sha256: RESULT_CONTRACT_SHA256,
        scenarios: [],
      }),
    ).toThrow(AssessmentContractError)
  })
})
