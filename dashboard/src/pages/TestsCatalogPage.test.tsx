import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { TestCatalogRow } from '@/lib/test-catalog'
import {
  CATALOG_DEFAULT_FILTERS,
  catalogCalibrationPresentation,
  catalogCountLabels,
  catalogFiltersActive,
  catalogFiltersFromParams,
  catalogFiltersToParams,
  catalogHorizonPresentation,
  catalogRealismPresentation,
  filterCatalogRows,
  groupCatalogRows,
  sortCatalogRows,
  TestsCatalogActions,
} from '@/pages/TestsCatalogPage'

function row(overrides: Partial<TestCatalogRow> = {}): TestCatalogRow {
  return {
    test_id: 'incident_response',
    lifecycle: 'active',
    current_version:
      'sha256:c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3',
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
    selected_version:
      'sha256:c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3',
    result: null,
    ...overrides,
  }
}

describe('test catalog dimensions', () => {
  it('keeps catalog total, loaded rows and filters distinct', () => {
    expect(catalogCountLabels(62, 50, 52, 12, true)).toEqual({
      summary:
        '62 catalog rows total · 50 loaded from catalog · 52 available in this view',
      visible: '12 of 52 available in this view',
      catalogProgress: '50 of 62 catalog rows loaded',
    })
    expect(catalogCountLabels(null, 0, 2, 2, false)).toEqual({
      summary: '2 available in this view',
      visible: '2 available in this view',
      catalogProgress: null,
    })
  })

  it('offers only plan and comparison entry points, never test authoring', () => {
    const html = renderToStaticMarkup(<TestsCatalogActions local />)

    expect(html).toContain('new plan')
    expect(html).toContain('compare versions')
    expect(html).not.toContain('new test')
    expect(html).not.toContain('Create a new local test')
  })

  it('presents horizon and realism independently', () => {
    expect(catalogHorizonPresentation(row())).toEqual({
      value: '60–120 min',
      detail: 'author estimate',
    })
    expect(catalogRealismPresentation(row())).toEqual({
      value: 'realistic simulator',
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
      expect(presentation.value).toBe('observed')
      expect(JSON.stringify(presentation).toLowerCase()).not.toContain('robust')
    }
  })

  // Audit T-12 / T-14: one marker for anything not declared.
  it('keeps absent dimensions compatible with older responses', () => {
    const legacy = row({
      characterization: undefined,
      calibration: undefined,
    })
    expect(catalogHorizonPresentation(legacy).value).toBeNull()
    expect(catalogRealismPresentation(legacy).value).toBeNull()
    expect(catalogCalibrationPresentation(legacy)).toEqual({
      value: 'no samples',
      detail: null,
    })
  })

  it('reads filters from the hash and writes only the non-default ones back', () => {
    const filters = catalogFiltersFromParams(
      new URLSearchParams(
        'q=chess&lifecycle=active&evidence=1&sort=runs&source=local',
      ),
    )
    expect(filters).toEqual({
      query: 'chess',
      lifecycle: 'active',
      realism: 'all',
      withExecutions: true,
      sort: 'runs',
    })
    expect(catalogFiltersToParams(filters).toString()).toBe(
      'q=chess&lifecycle=active&evidence=1&sort=runs',
    )
    expect(catalogFiltersActive(CATALOG_DEFAULT_FILTERS)).toBe(false)
  })

  it('groups by lifecycle and sorts by runs or last seen', () => {
    const rows = [
      row({ test_id: 'b', lifecycle: 'never_run' }),
      row({
        test_id: 'a',
        available_versions: [
          {
            version:
              'sha256:c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3',
            execution_count: 2,
            run_count: 2,
            last_seen: '2026-08-23T00:00:00Z',
          },
        ],
      }),
      row({
        test_id: 'c',
        lifecycle: 'retired',
        characterization: undefined,
      }),
    ]
    expect(
      groupCatalogRows(rows).map((group) => [
        group.lifecycle,
        group.rows.length,
      ]),
    ).toEqual([
      ['active', 1],
      ['never_run', 1],
      ['retired', 1],
    ])
    expect(sortCatalogRows(rows, 'runs').map((entry) => entry.test_id)).toEqual(
      ['a', 'b', 'c'],
    )
    expect(sortCatalogRows(rows, 'name').map((entry) => entry.test_id)).toEqual(
      ['a', 'b', 'c'],
    )
    expect(
      filterCatalogRows(rows, {
        ...CATALOG_DEFAULT_FILTERS,
        withExecutions: true,
      }).map((entry) => entry.test_id),
    ).toEqual(['a'])
    expect(
      filterCatalogRows(rows, {
        ...CATALOG_DEFAULT_FILTERS,
        realism: 'none',
      }).map((entry) => entry.test_id),
    ).toEqual(['c'])
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
    ).toBe('repeatable')
    expect(
      catalogCalibrationPresentation(
        row({
          calibration: {
            maturity: 'tail_calibrated',
            compatible_sample_count: 20,
          },
        }),
      ).value,
    ).toBe('tail calibrated')
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
      value: 'reference verified',
      detail: '0 compatible samples',
    })
  })
})
