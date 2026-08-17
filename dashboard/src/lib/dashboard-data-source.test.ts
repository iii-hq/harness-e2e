import { describe, expect, it } from 'vitest'
import {
  type StaticVersionSide,
  staticCompatibility,
  staticSideKey,
} from '@/lib/dashboard-data-source'

function side(
  assessment = 'assessment-a',
  analyzer: string | null = 'analyzer-a',
): StaticVersionSide {
  return {
    summary: {} as StaticVersionSide['summary'],
    contracts: { case: 'contract-a' },
    assessment_profiles: { case: assessment },
    analyzer_profiles: { case: analyzer },
  }
}

describe('static dashboard assessment parity', () => {
  it('keys retained comparison data by cohort and evaluated version', () => {
    expect(staticSideKey('cohort-a', 'version-a')).toBe('cohort-a::version-a')
  })

  it('keeps assessment and analyzer incompatibilities distinct', () => {
    expect(staticCompatibility(side(), side())).toEqual({
      compatibility: 'compatible',
      reasons: [],
    })
    expect(staticCompatibility(side(), side('assessment-b'))).toEqual({
      compatibility: 'assessment_changed',
      reasons: ['assessment_profile_changed'],
    })
    expect(staticCompatibility(side(), side('assessment-a', null))).toEqual({
      compatibility: 'analyzer_conflict',
      reasons: ['analyzer_profile_conflict'],
    })
  })
})
