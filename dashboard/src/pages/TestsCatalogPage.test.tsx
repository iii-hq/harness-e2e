import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { LocalScenarioSummary } from '@/lib/local-scenario-catalog'
import type { TestCatalogRow } from '@/lib/test-catalog'
import {
  catalogCalibrationPresentation,
  catalogComplexityPresentation,
  catalogHorizonPresentation,
  catalogRealismPresentation,
  LocalTestBadge,
  mergeLocalScenariosIntoCatalog,
  nextLocalScenarioFileName,
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

    expect(html).toContain('New test')
    expect(html).toContain('Create a new local test')
    expect(html).toContain('New plan')
    expect(html).toContain('Compare')
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
