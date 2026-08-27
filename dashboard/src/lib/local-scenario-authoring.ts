export type LocalScenarioValidationDraft = {
  id: string
  title: string
  weight: string
  instructions: string
}

export type LocalScenarioDraft = {
  title: string
  version: string
  beforeTest: string
  prompt: string
  validations: LocalScenarioValidationDraft[]
}

type MarkdownHeading = {
  level: number
  title: string
  lineStart: number
  bodyStart: number
}

const REQUIRED_SECTIONS = [
  'Plans',
  'Version',
  'Before Test',
  'Prompt',
  'Validations',
] as const

export const INITIAL_LOCAL_SCENARIO_DRAFT: LocalScenarioDraft = {
  title: 'Local scenario',
  version: '1',
  beforeTest:
    'Prepare the isolated state required by this test. Keep every mutation run-scoped and reversible.',
  prompt: 'Describe the task the Harness must complete.',
  validations: [
    {
      id: 'validation-1',
      title: 'Expected outcome',
      weight: '70',
      instructions:
        'Describe the evidence that proves the requested outcome is correct.',
    },
    {
      id: 'validation-2',
      title: 'Safe execution',
      weight: '30',
      instructions:
        'Confirm the run stayed within the intended scope and left no residual state.',
    },
  ],
}

function normalizeLineEndings(source: string) {
  return source.replace(/\r\n?/g, '\n')
}

function scanHeadings(source: string): MarkdownHeading[] {
  const headings: MarkdownHeading[] = []
  let offset = 0
  let fence: '`' | '~' | null = null

  for (const lineWithNewline of source.split(/(?<=\n)/)) {
    if (lineWithNewline === '') continue
    const line = lineWithNewline.endsWith('\n')
      ? lineWithNewline.slice(0, -1)
      : lineWithNewline
    const trimmed = line.trimStart()
    const fenceMatch = /^(`{3,}|~{3,})/.exec(trimmed)
    if (fenceMatch) {
      const marker = fenceMatch[1][0] as '`' | '~'
      if (fence === marker) fence = null
      else if (fence === null) fence = marker
      offset += lineWithNewline.length
      continue
    }
    if (fence === null) {
      const heading = /^(#{1,6}) (.*)$/.exec(line)
      if (heading) {
        headings.push({
          level: heading[1].length,
          title: heading[2].trim(),
          lineStart: offset,
          bodyStart: offset + lineWithNewline.length,
        })
      }
    }
    offset += lineWithNewline.length
  }

  if (fence !== null)
    throw new Error('The Markdown contains an unclosed fenced code block.')
  return headings
}

function sectionBody(
  source: string,
  headings: MarkdownHeading[],
  title: string,
) {
  const index = headings.findIndex(
    (heading) => heading.level === 2 && heading.title === title,
  )
  if (index < 0) throw new Error(`Missing section ## ${title}.`)
  const heading = headings[index]
  const next = headings
    .slice(index + 1)
    .find((candidate) => candidate.level <= 2)
  return source
    .slice(heading.bodyStart, next?.lineStart ?? source.length)
    .trim()
}

function safeSlug(value: string) {
  const normalized = value.trim().toLowerCase().replace(/[- ]/g, '_')
  if (
    normalized === '' ||
    normalized.startsWith('_') ||
    normalized.includes('__') ||
    !/^[a-z0-9_]+$/.test(normalized)
  ) {
    return null
  }
  return normalized
}

function parsePositiveInteger(value: string, label: string, max: number) {
  const trimmed = value.trim()
  if (!/^[1-9]\d*$/.test(trimmed) || Number(trimmed) > max)
    throw new Error(`${label} must be a positive integer.`)
  return trimmed
}

function markdownBodyIssue(
  value: string,
  label: string,
  lowestUnsupportedHeading: number,
) {
  let headings: MarkdownHeading[]
  try {
    headings = scanHeadings(normalizeLineEndings(value))
  } catch (cause) {
    return `${label}: ${cause instanceof Error ? cause.message : String(cause)}`
  }
  if (headings.some((heading) => heading.level <= lowestUnsupportedHeading))
    return `${label} cannot contain an H1${lowestUnsupportedHeading === 3 ? ', H2 or H3' : ' or H2'} heading outside a code fence.`

  let remainder = value
  while (remainder.includes('{{')) {
    const open = remainder.indexOf('{{')
    const afterOpen = remainder.slice(open + 2)
    const close = afterOpen.indexOf('}}')
    if (close < 0) return `${label} contains an unclosed template variable.`
    const variable = afterOpen.slice(0, close).trim()
    if (variable !== 'run_id' && variable !== 'seed')
      return `${label} references unsupported template variable “${variable}”.`
    remainder = afterOpen.slice(close + 2)
  }
  return null
}

export function localScenarioValidationWeight(draft: LocalScenarioDraft) {
  return draft.validations.reduce((total, validation) => {
    const weight = Number(validation.weight)
    return total + (Number.isInteger(weight) ? weight : 0)
  }, 0)
}

export function localScenarioDraftIssue(draft: LocalScenarioDraft) {
  if (draft.title.trim() === '') return 'Add a test name.'
  if (!/^[1-9]\d*$/.test(draft.version.trim()))
    return 'Version must be a positive integer.'
  if (Number(draft.version) > 4_294_967_295)
    return 'Version must fit within an unsigned 32-bit integer.'
  if (draft.beforeTest.trim() === '')
    return 'Describe the state that must exist before the test.'
  if (draft.prompt.trim() === '') return 'Describe the task for the Harness.'
  const beforeTestIssue = markdownBodyIssue(draft.beforeTest, 'Before test', 2)
  if (beforeTestIssue) return beforeTestIssue
  const promptIssue = markdownBodyIssue(draft.prompt, 'Task prompt', 2)
  if (promptIssue) return promptIssue
  if (draft.validations.length === 0)
    return 'Add at least one validation criterion.'

  const criterionIds = new Set<string>()
  for (const [index, validation] of draft.validations.entries()) {
    const position = index + 1
    if (validation.title.trim() === '')
      return `Add a name for validation ${position}.`
    const criterionId = safeSlug(validation.title)
    if (!criterionId)
      return `Validation ${position} name must use letters, numbers, spaces, hyphens or underscores.`
    if (criterionIds.has(criterionId))
      return 'Validation names must compile to unique identifiers.'
    criterionIds.add(criterionId)
    if (!/^[1-9]\d*$/.test(validation.weight.trim()))
      return `Validation ${position} weight must be a positive integer.`
    if (Number(validation.weight) > 255)
      return `Validation ${position} weight must be 255 or less.`
    if (validation.instructions.trim() === '')
      return `Describe how validation ${position} will be evaluated.`
    const instructionsIssue = markdownBodyIssue(
      validation.instructions,
      `Validation ${position} instructions`,
      3,
    )
    if (instructionsIssue) return instructionsIssue
  }

  const weight = localScenarioValidationWeight(draft)
  if (weight !== 100)
    return `Validation weights total ${weight}%; adjust them to exactly 100%.`
  return null
}

export function buildLocalScenarioSource(draft: LocalScenarioDraft) {
  const validations = draft.validations.flatMap((validation) => [
    `### ${validation.title.trim()} (${validation.weight.trim()}%)`,
    validation.instructions.trim(),
  ])
  return [
    `# ${draft.title.trim()}`,
    '## Plans',
    '- local',
    '## Version',
    draft.version.trim(),
    '## Before Test',
    draft.beforeTest.trim(),
    '## Prompt',
    draft.prompt.trim(),
    '## Validations',
    ...validations,
  ]
    .join('\n\n')
    .concat('\n')
}

export const LOCAL_SCENARIO_TEMPLATE = buildLocalScenarioSource(
  INITIAL_LOCAL_SCENARIO_DRAFT,
)

export function parseLocalScenarioSource(
  rawSource: string,
): LocalScenarioDraft {
  const source = normalizeLineEndings(rawSource)
  if (source.trim() === '') throw new Error('The Markdown file is empty.')
  if (source.startsWith('---\n'))
    throw new Error('YAML frontmatter is not supported.')

  const headings = scanHeadings(source)
  const titles = headings.filter((heading) => heading.level === 1)
  if (titles.length !== 1)
    throw new Error('The Markdown must contain exactly one H1 test name.')
  const title = titles[0].title.trim()
  if (title === '') throw new Error('The H1 test name cannot be empty.')

  const sections = headings
    .filter((heading) => heading.level === 2)
    .map((heading) => heading.title)
  if (
    sections.length !== REQUIRED_SECTIONS.length ||
    sections.some((section, index) => section !== REQUIRED_SECTIONS[index])
  ) {
    throw new Error(
      `Sections must appear once in this order: ${REQUIRED_SECTIONS.join(', ')}.`,
    )
  }

  const plans = sectionBody(source, headings, 'Plans')
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean)
  if (plans.length !== 1 || plans[0] !== '- local')
    throw new Error('Local tests must contain only “- local” under Plans.')

  const version = parsePositiveInteger(
    sectionBody(source, headings, 'Version'),
    'Version',
    4_294_967_295,
  )
  const beforeTest = sectionBody(source, headings, 'Before Test')
  const prompt = sectionBody(source, headings, 'Prompt')
  if (beforeTest === '' || prompt === '')
    throw new Error('Before Test and Prompt cannot be empty.')

  const validationsHeading = headings.find(
    (heading) => heading.level === 2 && heading.title === 'Validations',
  )
  if (!validationsHeading) throw new Error('Missing section ## Validations.')
  const nextSection = headings.find(
    (heading) =>
      heading.lineStart > validationsHeading.lineStart && heading.level <= 2,
  )
  const validationsEnd = nextSection?.lineStart ?? source.length
  const criteria = headings.filter(
    (heading) =>
      heading.level === 3 &&
      heading.lineStart > validationsHeading.lineStart &&
      heading.lineStart < validationsEnd,
  )
  if (criteria.length === 0)
    throw new Error('Add at least one H3 validation criterion.')

  const validations = criteria.map((criterion, index) => {
    const match = /^(.*) \((\d+)%\)$/.exec(criterion.title)
    if (!match || match[1].trim() === '')
      throw new Error(`Validation ${index + 1} heading must end in “(N%)”.`)
    const weight = parsePositiveInteger(
      match[2],
      `Validation ${index + 1} weight`,
      255,
    )
    const end = criteria[index + 1]?.lineStart ?? validationsEnd
    const instructions = source.slice(criterion.bodyStart, end).trim()
    if (instructions === '')
      throw new Error(`Validation ${index + 1} instructions cannot be empty.`)
    return {
      id: `validation-${index + 1}`,
      title: match[1].trim(),
      weight,
      instructions,
    }
  })

  const draft = { title, version, beforeTest, prompt, validations }
  const issue = localScenarioDraftIssue(draft)
  if (issue) throw new Error(issue)
  return draft
}
