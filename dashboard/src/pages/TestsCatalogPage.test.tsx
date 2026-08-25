import { describe, expect, it } from 'vitest'
import type { TestCatalogRow } from '@/lib/test-catalog'
import {
  catalogCalibrationPresentation,
  catalogComplexityPresentation,
  catalogHorizonPresentation,
  catalogRealismPresentation,
} from '@/pages/TestsCatalogPage'

function row(overrides: Partial<TestCatalogRow> = {}): TestCatalogRow {
  return {
    test_id: 'incident_response',
    lifecycle: 'active',
    current_version: 3,
    complexity: {
      method: 'capability_v2',
      tier: 'l5_adaptive',
    },
    characterization: {
      human_horizon: {
        min_minutes: 60,
        max_minutes: 120,
        basis: 'author_estimate',
      },
      realism: {
        execution: 'realistic_simulator',
        shadow: 'read_only',
      },
    },
    calibration: {
      maturity: 'observed',
      compatible_sample_count: 4,
    },
    available_versions: [],
    selected_version: 3,
    result: null,
    ...overrides,
  }
}

describe('test catalog L5 dimensions', () => {
  it('presents classification, horizon, and realism independently', () => {
    expect(catalogComplexityPresentation(row())).toEqual({
      value: 'L5 adaptive',
      detail: 'capability v2',
    })
    expect(catalogHorizonPresentation(row())).toEqual({
      value: '60–120 min',
      detail: 'author estimate',
    })
    expect(catalogRealismPresentation(row())).toEqual({
      value: 'Realistic simulator',
      detail: 'read-only shadow',
    })
  })

  it('calls one to four compatible samples observed, never robust', () => {
    for (const compatible_sample_count of [1, 4]) {
      const presentation = catalogCalibrationPresentation(
        row({
          calibration: {
            maturity: 'observed',
            compatible_sample_count,
          },
        }),
      )
      expect(presentation.value).toBe('Observed')
      expect(JSON.stringify(presentation).toLowerCase()).not.toContain('robust')
    }
  })

  it('keeps absent dimensions compatible with older responses', () => {
    const legacy = row({
      complexity: undefined,
      characterization: undefined,
      calibration: undefined,
    })
    expect(catalogComplexityPresentation(legacy).value).toBe('Not declared')
    expect(catalogHorizonPresentation(legacy).value).toBe('Unknown')
    expect(catalogRealismPresentation(legacy).value).toBe('Not declared')
    expect(catalogCalibrationPresentation(legacy)).toEqual({
      value: 'Candidate',
      detail: null,
    })
  })

  it('distinguishes repeatability from tail calibration', () => {
    expect(
      catalogCalibrationPresentation(
        row({
          calibration: {
            maturity: 'repeatable',
            compatible_sample_count: 19,
          },
        }),
      ).value,
    ).toBe('Repeatable')
    expect(
      catalogCalibrationPresentation(
        row({
          calibration: {
            maturity: 'tail_calibrated',
            compatible_sample_count: 20,
          },
        }),
      ).value,
    ).toBe('Tail calibrated')
  })

  it('shows a verified reference before live calibration samples exist', () => {
    expect(
      catalogCalibrationPresentation(
        row({
          calibration: {
            maturity: 'reference_verified',
            compatible_sample_count: 0,
          },
        }),
      ),
    ).toEqual({
      value: 'Reference verified',
      detail: '0 compatible samples',
    })
  })
})
