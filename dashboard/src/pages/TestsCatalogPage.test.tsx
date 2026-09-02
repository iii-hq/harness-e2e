import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { LocalScenarioSummary } from '@/lib/local-scenario-catalog'
import type { TestCatalogRow } from '@/lib/test-catalog'
import {
  CATALOG_DEFAULT_FILTERS,
  catalogCalibrationPresentation,
  catalogComplexityPresentation,
  catalogFiltersActive,
  catalogFiltersFromParams,
  catalogFiltersToParams,
  catalogHorizonPresentation,
  catalogRealismPresentation,
  filterCatalogRows,
  groupCatalogRows,
  LocalTestBadge,
  mergeLocalScenariosIntoCatalog,
  nextLocalScenarioFileName,
  sortCatalogDisplayRows,
  TestsCatalogActions,
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

function localScenario(
  overrides: Partial<LocalScenarioSummary> = {},
): LocalScenarioSummary {
  return {
    id: 'local_example',
    title: 'Local example',
    version: 1,
    source_path: 'local-scenarios/example.md',
    source_sha256: 'sha256:test',
    ...overrides,
  }
}

describe('test catalog L5 dimensions', () => {
  it('places local test creation in the actual Tests catalog actions', () => {
    const html = renderToStaticMarkup(
      <TestsCatalogActions local localReady onNewTest={() => undefined} />,
    )

    expect(html).toContain('new test')
    expect(html).toContain('Create a new local test')
    expect(html).toContain('new plan')
    expect(html).toContain('compare versions')
    expect(html).not.toContain('disabled=""')
  })

  it('suggests a new file name instead of colliding with a saved test', () => {
    expect(nextLocalScenarioFileName([])).toBe('new-test.md')
    expect(
      nextLocalScenarioFileName([
        localScenario({
          source_path: 'local-scenarios/new-test.md',
        }),
        localScenario({
          source_path: 'local-scenarios/new-test-2.md',
        }),
      ]),
    ).toBe('new-test-3.md')
  })

  it('lists local definitions as normal catalog rows without duplicates', () => {
    const local = localScenario()
    const [synthetic] = mergeLocalScenariosIntoCatalog([], [local])
    expect(synthetic).toMatchObject({
      localScenario: local,
      row: {
        test_id: 'local_example',
        lifecycle: 'never_run',
        current_version: 1,
        available_versions: [{ version: 1, execution_count: 0 }],
      },
    })

    const executed = row({
      test_id: local.id,
      lifecycle: 'retired',
      current_version: null,
      available_versions: [
        {
          version: local.version,
          execution_count: 1,
          run_count: 1,
          last_seen: '2026-08-26T19:53:13.315Z',
        },
      ],
    })
    const merged = mergeLocalScenariosIntoCatalog([executed], [local])
    expect(merged).toHaveLength(1)
    expect(merged[0]).toMatchObject({
      localScenario: local,
      row: {
        test_id: local.id,
        lifecycle: 'active',
        current_version: local.version,
      },
    })
  })

  // Audit T-09 / DS-07: the badge speaks the same vocabulary as the
  // version pill — mono, lowercase, 6px, fill, no border, 11px.
  it('renders the local-origin badge in the mono vocabulary', () => {
    const html = renderToStaticMarkup(<LocalTestBadge />)
    expect(html).toContain('>local<')
    expect(html).toContain('bg-[var(--surface-fill)]')
    expect(html).toContain('rounded-[6px]')
    expect(html).not.toContain('rounded-full')
    expect(html).not.toContain('border-brand')
    expect(html).not.toContain('uppercase')
  })

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
      complexity: undefined,
      characterization: undefined,
      calibration: undefined,
    })
    expect(catalogComplexityPresentation(legacy).value).toBeNull()
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
        'q=chess&lifecycle=active&evidence=1&sort=runs&source=nope',
      ),
    )
    expect(filters).toMatchObject({
      query: 'chess',
      lifecycle: 'active',
      withExecutions: true,
      sort: 'runs',
      source: 'all',
    })
    expect(catalogFiltersToParams(filters).toString()).toBe(
      'q=chess&lifecycle=active&evidence=1&sort=runs',
    )
    expect(catalogFiltersActive(CATALOG_DEFAULT_FILTERS)).toBe(false)
  })

  it('groups by lifecycle and sorts by runs, last seen or complexity', () => {
    const rows = [
      {
        row: row({
          test_id: 'b',
          lifecycle: 'never_run',
          complexity: { tier: 'l2_stateful' },
        }),
        localScenario: null,
      },
      {
        row: row({
          test_id: 'a',
          available_versions: [
            {
              version: 3,
              execution_count: 2,
              run_count: 2,
              last_seen: '2026-08-23T00:00:00Z',
            },
          ],
        }),
        localScenario: null,
      },
      {
        row: row({ test_id: 'c', lifecycle: 'retired', complexity: null }),
        localScenario: null,
      },
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
    expect(
      sortCatalogDisplayRows(rows, 'runs').map((entry) => entry.row.test_id),
    ).toEqual(['a', 'b', 'c'])
    expect(
      sortCatalogDisplayRows(rows, 'complexity').map(
        (entry) => entry.row.test_id,
      ),
    ).toEqual(['a', 'b', 'c'])
    expect(
      filterCatalogRows(rows, {
        ...CATALOG_DEFAULT_FILTERS,
        withExecutions: true,
      }).map((entry) => entry.row.test_id),
    ).toEqual(['a'])
    expect(
      filterCatalogRows(rows, {
        ...CATALOG_DEFAULT_FILTERS,
        complexity: 'none',
      }).map((entry) => entry.row.test_id),
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
