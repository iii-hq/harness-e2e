import { describe, expect, it } from 'vitest'
import { definitionChoices, staleNotice } from '@/lib/test-history'
import { formFlow, history } from '@/test-fixtures/test-history'

const C = `sha256:${'c3'.repeat(32)}`

describe('definitions', () => {
  it('lists the current definition first, then the most recently run', () => {
    expect(
      definitionChoices(formFlow).map((choice) => [
        choice.label,
        choice.runs,
        choice.current,
      ]),
    ).toEqual([
      ['1768da69', 1, true],
      ['0c5c0902', 4, false],
      ['6831b628', 1, false],
    ])
  })

  it('says when the current definition has not run yet', () => {
    expect(staleNotice(formFlow)).toBeNull()
    const moved = history({
      current_version: C,
      available_versions: [
        ...formFlow.available_versions,
        { version: C, execution_count: 0, run_count: 0, last_seen: null },
      ],
    })
    expect(definitionChoices(moved)[0]).toMatchObject({
      label: 'c3c3c3c3',
      runs: 0,
      current: true,
    })
    expect(staleNotice(moved)).toBe(
      'The current definition c3c3c3c3 hasn’t run yet. The 6 runs below belong to 3 earlier definitions, so they don’t say how the test behaves now.',
    )
    // A test that never ran has nothing below to warn about.
    expect(
      staleNotice(
        history({
          current_version: C,
          available_versions: [
            { version: C, execution_count: 0, run_count: 0, last_seen: null },
          ],
        }),
      ),
    ).toBeNull()
  })
})
