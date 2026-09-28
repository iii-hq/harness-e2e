import { afterEach, describe, expect, it, vi } from 'vitest'
import { copyText } from '@/lib/clipboard'

afterEach(() => vi.unstubAllGlobals())

describe('copyText', () => {
  it('uses the clipboard where the page has it', async () => {
    const writeText = vi.fn(async () => {})
    vi.stubGlobal('navigator', { clipboard: { writeText } })
    expect(await copyText('plan-1')).toBe(true)
    expect(writeText).toHaveBeenCalledWith('plan-1')
  })

  it('falls back to the copy command on an http page, beside what has focus', async () => {
    vi.stubGlobal('navigator', {})
    const appended: { value: string }[] = []
    const button = {
      focus: vi.fn(),
      parentElement: {
        append: (area: { value: string }) => appended.push(area),
      },
    }
    const area = {
      value: '',
      style: {},
      setAttribute: () => {},
      select: vi.fn(),
      remove: vi.fn(),
    }
    const execCommand = vi.fn(() => true)
    vi.stubGlobal('document', {
      activeElement: button,
      body: { append: () => {} },
      createElement: () => area,
      execCommand,
    })
    expect(await copyText('kind: stack')).toBe(true)
    expect(appended[0].value).toBe('kind: stack')
    expect(execCommand).toHaveBeenCalledWith('copy')
    expect(area.remove).toHaveBeenCalled()
    expect(button.focus).toHaveBeenCalled()
  })

  it('says false when neither works', async () => {
    vi.stubGlobal('navigator', {})
    expect(await copyText('x')).toBe(false)
  })
})
