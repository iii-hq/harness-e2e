import {
  ConfirmDialog,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
  SegmentedControl,
} from '@iii-dev/console-ui'
import {
  ChevronDown,
  CircleX,
  Copy,
  Lock,
  Plus,
  Search,
  TriangleAlert,
  X,
} from 'lucide-react'
import {
  type CSSProperties,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react'
import { copyText } from '@/lib/clipboard'
import type {
  DashboardDataBridge,
  IiiReleases,
  Stack,
  StackPreview,
  StackTemplates,
} from '@/lib/dashboard-data-source'
import { formatDateTime, plural } from '@/lib/format'
import {
  addedWhenItRuns,
  type Declared,
  declaredOf,
  judge,
  type Lookups,
  lookupOf,
  orderTemplates,
  packageName,
  pickerGroups,
  pinLabel,
  type Source,
  templateExtra,
  typedName,
  type Verdict,
  yamlOf,
} from '@/lib/stack-builder'
import {
  belowMinimum,
  blockedLines,
  containerOf,
  formLocks,
  iiiChoice,
  iiiLabel,
  iiiRefusal,
  type Locks,
  lockedAll,
  patchContainers,
  type Read,
  readOf,
  releasedOn,
  setScalar,
  sheetBlocks,
  sheetStatus,
  splitTemplate,
  type Tab,
  templateRef,
} from '@/lib/stack-sheet'
import {
  iiiHint,
  savedStatus,
  stackDeclares,
  stackFile,
  warningsTitle,
  yamlLine,
  yamlMeta,
} from '@/lib/stacks-view'
import './stack-sheet.css'

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

type Templates =
  | { state: 'loading' }
  | { state: 'ready'; read: StackTemplates }
  | { state: 'failed'; message: string }

type IiiCheck = IiiReleases['checked'] | { error: string }

const SOURCES: Array<{ id: Source; label: string }> = [
  { id: 'none', label: 'No template' },
  { id: 'template', label: 'An iii-hq/templates project' },
  { id: 'copy', label: 'Copy of a stack' },
]

const TAB_KEY = 'harness-e2e.stack-sheet.tab'
const CREATED = 'Created. Change what you need, then save.'
/** A revision GitHub answered it does not hold, or one that isn't one:
 *  what blocks. Anything else it did not answer blocks nothing. */
const UNRESOLVED =
  /^(iii-hq\/templates has no commit, tag or branch|A revision is)/

function rememberedTab(): Tab {
  try {
    return window.localStorage.getItem(TAB_KEY) === 'yaml' ? 'yaml' : 'form'
  } catch {
    return 'form'
  }
}

function rememberTab(tab: Tab) {
  try {
    window.localStorage.setItem(TAB_KEY, tab)
  } catch {
    // A Console without storage opens on the Form next time.
  }
}

/** One screen to create, edit and view a stack (StackSheet): a name, the
 *  Form and YAML tabs over one draft, the YAML text, and Create or Save held
 *  back with the reason in both. The Form reads the draft through the
 *  runner's preview and writes back line by line; a repository stack opens
 *  read-only in both tabs, with Copy to edit. */
export function StackSheet({
  mode,
  stack,
  stacks,
  narrow,
  bridge,
  busy = false,
  copyError = null,
  created = false,
  gone = false,
  onCreated,
  onSaved,
  onCopy,
  onDelete,
  onClose,
}: {
  mode: 'new' | 'edit' | 'view'
  /** The stack edited or viewed; null for a new one. */
  stack: Stack | null
  stacks: Stack[]
  narrow: boolean
  bridge: DashboardDataBridge | null
  /** A copy in flight (Copy to edit). */
  busy?: boolean
  copyError?: string | null
  /** Opened right after it was created. */
  created?: boolean
  /** No longer listed: deleted elsewhere while it was open. */
  gone?: boolean
  onCreated?: (stack: Stack) => void
  onSaved?: (stack: Stack) => void
  onCopy?: () => void
  onDelete?: () => void
  onClose: () => void
}) {
  const viewing = mode === 'view'
  const [initial] = useState(() =>
    stack
      ? { label: stack.label, yaml: stack.yaml, read: readOf(stack) }
      : {
          label: '',
          yaml: `${yamlOf('latest', null, []).join('\n')}\n`,
          read: {
            iii: 'latest',
            template: null,
            containers: [],
            warnings: [],
          } satisfies Read,
        },
  )
  const [name, setName] = useState(initial.label)
  const named = useRef(false)
  const [yaml, setYaml] = useState(initial.yaml)
  const [tab, setTab] = useState<Tab>(rememberedTab)
  // What the runner read of a draft, and what a form change is known to
  // read as until it answers.
  const [seen, setSeen] = useState<{ yaml: string; preview: StackPreview }>(
    () =>
      viewing
        ? { yaml: initial.yaml, preview: initial.read }
        : { yaml: '', preview: { refused: '' } },
  )
  const [optimistic, setOptimistic] = useState<{
    yaml: string
    read: Read
  } | null>(() => ({ yaml: initial.yaml, read: initial.read }))
  const [source, setSource] = useState<Source>('template')
  const [copy, setCopy] = useState<Stack | null>(null)
  const [picking, setPicking] = useState(mode === 'new')
  const [templates, setTemplates] = useState<Record<string, Templates>>({})
  const [lookups, setLookups] = useState<Lookups>({})
  const [releases, setReleases] = useState<
    | { state: 'loading' }
    | { state: 'ready'; read: IiiReleases }
    | { state: 'failed'; message: string }
  >({ state: 'loading' })
  const [checks, setChecks] = useState<Record<string, IiiCheck>>({})
  const [iiiOpen, setIiiOpen] = useState(false)
  const [menu, setMenu] = useState<string | null>(null)
  const [picker, setPicker] = useState(false)
  const [query, setQuery] = useState('')
  const [showAll, setShowAll] = useState(false)
  const [saving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)
  const [said, setSaid] = useState(created ? CREATED : '')
  const [copied, setCopied] = useState(false)
  const [asking, setAsking] = useState(false)
  const menuButton = useRef<HTMLButtonElement | null>(null)
  const iiiButton = useRef<HTMLButtonElement>(null)
  const addButton = useRef<HTMLButtonElement>(null)

  // The draft as the runner reads it: asked once typing stops; an answer
  // for a draft since changed is dropped.
  const latest = useRef(yaml)
  latest.current = yaml
  useEffect(() => {
    if (!bridge || viewing || seen.yaml === yaml) return
    const timer = setTimeout(() => {
      bridge.previewStack(yaml).then(
        (preview) => {
          if (latest.current === yaml) setSeen({ yaml, preview })
        },
        (cause) => {
          if (latest.current === yaml)
            setSeen({
              yaml,
              preview: {
                refused: `It couldn’t be read: ${errorText(cause)}`,
              },
            })
        },
      )
    }, 250)
    return () => clearTimeout(timer)
  }, [bridge, viewing, yaml, seen.yaml])

  const answered = seen.yaml === yaml ? seen.preview : null
  const refused = answered && 'refused' in answered ? answered.refused : null
  const read: Read | null =
    answered && !('refused' in answered)
      ? answered
      : !answered && optimistic?.yaml === yaml
        ? optimistic.read
        : null
  const pending = !read && refused === null
  const lastRead = useRef(initial.read)
  if (read) lastRead.current = read
  const shown = read ?? lastRead.current
  const declared = useMemo(() => declaredOf(shown), [shown])
  const template = templateRef(shown.template)
  const revision = shown.template?.revision ?? ''
  const revisionKey = revision || 'main'
  const locks: Locks = viewing
    ? lockedAll('Read-only.')
    : refused !== null
      ? lockedAll('The YAML doesn’t read. Fix it in the YAML tab.')
      : pending
        ? lockedAll('Reading the YAML…')
        : formLocks(yaml, shown)

  // iii-hq/templates at the revision the draft names (main when none),
  // read once per revision; a typed one waits for typing to stop.
  const loadTemplates = useCallback(
    (key: string) => {
      if (!bridge) return
      setTemplates((current) => ({ ...current, [key]: { state: 'loading' } }))
      bridge.listStackTemplates(key === 'main' ? undefined : key).then(
        (answer) =>
          setTemplates((current) => ({
            ...current,
            [key]: { state: 'ready', read: answer },
          })),
        (cause) =>
          setTemplates((current) => ({
            ...current,
            [key]: { state: 'failed', message: errorText(cause) },
          })),
      )
    },
    [bridge],
  )
  const wantTemplates = mode === 'new' || Boolean(template) || picking
  useEffect(() => {
    if (!wantTemplates || templates[revisionKey]) return
    const timer = setTimeout(
      () => loadTemplates(revisionKey),
      revisionKey === 'main' ? 0 : 400,
    )
    return () => clearTimeout(timer)
  }, [wantTemplates, templates, revisionKey, loadTemplates])
  const at = templates[revisionKey] ?? { state: 'loading' as const }
  const listed = useMemo(
    () => orderTemplates(at.state === 'ready' ? at.read.templates : []),
    [at],
  )
  const tpl = listed.find((entry) => entry.id === shown.template?.id) ?? null
  const revisionError =
    revision && at.state === 'failed' && UNRESOLVED.test(at.message)
      ? at.message
      : null

  // The iii releases, and a version the draft names that they don't list.
  const loadReleases = useCallback(() => {
    if (!bridge) return
    setReleases({ state: 'loading' })
    bridge.listIiiReleases().then(
      (answer) => setReleases({ state: 'ready', read: answer }),
      (cause) => setReleases({ state: 'failed', message: errorText(cause) }),
    )
  }, [bridge])
  useEffect(() => {
    if (!viewing) loadReleases()
  }, [viewing, loadReleases])
  const known = releases.state === 'ready' ? releases.read : null
  const unlisted =
    known &&
    shown.iii &&
    shown.iii !== 'latest' &&
    !known.releases.some((release) => release.version === shown.iii)
      ? shown.iii
      : null
  useEffect(() => {
    if (!bridge || !unlisted || checks[unlisted]) return
    const timer = setTimeout(() => {
      bridge.listIiiReleases(unlisted).then(
        (answer) =>
          setChecks((current) => ({
            ...current,
            [unlisted]: answer.checked,
          })),
        (cause) =>
          setChecks((current) => ({
            ...current,
            [unlisted]: { error: errorText(cause) },
          })),
      )
    }, 400)
    return () => clearTimeout(timer)
  }, [bridge, unlisted, checks])
  const unlistedCheck = unlisted ? checks[unlisted] : undefined
  const iiiWarning =
    unlistedCheck && 'error' in unlistedCheck
      ? unlistedCheck.error
      : unlistedCheck
        ? iiiRefusal(unlistedCheck)
        : null
  const minimum = belowMinimum(shown.iii, known, tpl)

  // The registry, asked once per worker while it is answered or being asked.
  const asked = useRef(new Set<string>())
  const check = useCallback(
    (worker: string) => {
      if (!bridge || asked.current.has(worker)) return
      asked.current.add(worker)
      setLookups((current) => ({ ...current, [worker]: { state: 'checking' } }))
      bridge.resolveWorker(worker).then(
        (resolution) => {
          const answer = lookupOf(resolution)
          if (answer.state === 'failed') asked.current.delete(worker)
          setLookups((current) => ({ ...current, [worker]: answer }))
        },
        (cause) => {
          asked.current.delete(worker)
          setLookups((current) => ({
            ...current,
            [worker]: { state: 'failed', message: errorText(cause) },
          }))
        },
      )
    },
    [bridge],
  )
  useEffect(() => {
    if (template) return
    for (const entry of declared) {
      const own = packageName(entry.worker)
      if (own) check(own)
    }
  }, [template, declared, check])

  const verdicts = declared.map((entry) =>
    judge(entry, template, tpl, declared, lookups),
  )
  const blocks = sheetBlocks({
    name,
    refused,
    revisionError,
    declared,
    verdicts,
  })
  const blockedNames = new Set(
    blocks.flatMap((block) => (block.container ? [block.container] : [])),
  )
  const status = sheetStatus({
    name,
    source: mode === 'new' ? source : template ? 'template' : 'none',
    template,
    tpl,
    declared,
    verdicts,
    verb: mode === 'new' ? 'create' : 'save',
    pending,
    refused,
    revisionError,
  })
  // What it was saved as (or, new, what it opened as): a save moves it.
  const base = {
    label: stack?.label ?? initial.label,
    yaml: stack?.yaml ?? initial.yaml,
  }
  const dirty =
    !viewing && (name.trim() !== base.label.trim() || yaml !== base.yaml)
  const exact =
    mode === 'new' && source === 'copy' && copy !== null && yaml === copy.yaml
  const groups = pickerGroups({
    query,
    template,
    tpl,
    declared,
    lookups,
    stacks,
    templates: listed,
    showAll,
  })
  const typed = picker ? typedName(query, groups) : null
  useEffect(() => {
    if (!typed) return
    const timer = setTimeout(() => check(typed), 300)
    return () => clearTimeout(timer)
  }, [typed, check])

  // An open menu closes on a click outside it.
  useEffect(() => {
    if (!menu && !iiiOpen) return
    const outside = (event: PointerEvent) => {
      const target = event.target as Element | null
      if (target?.closest?.('[data-version-menu], [data-pin], [data-iii-menu]'))
        return
      setMenu(null)
      setIiiOpen(false)
    }
    document.addEventListener('pointerdown', outside, true)
    return () => document.removeEventListener('pointerdown', outside, true)
  }, [menu, iiiOpen])

  /** The draft rewritten, and what it reads as until the runner answers. */
  const write = (next: string, reads: Read | null) => {
    setYaml(next)
    setCopied(false)
    setSaveError(null)
    setSaid('')
    setOptimistic(reads ? { yaml: next, read: reads } : null)
  }
  /** A form change made to the draft's lines; nothing when they can't be
   *  patched (the field says so). */
  const change = (next: {
    iii?: string
    template?: string | null
    declared?: Declared[]
  }) => {
    if (!read) return
    let text = yaml
    if (next.iii !== undefined) text = setScalar(text, 'iii', next.iii)
    if (next.template !== undefined)
      text = setScalar(text, 'template', next.template)
    if (next.declared) {
      const patched = patchContainers(text, declared, next.declared)
      if (patched === null) return
      text = patched
    }
    write(text, {
      ...read,
      iii: next.iii ?? read.iii,
      template:
        next.template !== undefined
          ? splitTemplate(next.template)
          : read.template,
      containers: next.declared
        ? next.declared.map(containerOf)
        : read.containers,
    })
  }
  const nameAfter = (next: string) => {
    if (!named.current) setName(next)
  }
  const pickSource = (next: Source) => {
    setSource(next)
    setMenu(null)
    setPicking(next === 'template')
    if (next === 'none' && template) change({ template: null })
  }
  const pickTemplate = (id: string | null) => {
    setPicking(false)
    setShowAll(false)
    change({
      template: id && templateRef({ id, revision: revision || null }),
    })
    if (id && !(mode === 'new' && source === 'copy')) nameAfter(id)
  }
  const pickCopy = (other: Stack) => {
    setCopy(other)
    setMenu(null)
    write(other.yaml, readOf(other))
    nameAfter(`${other.label} · copy`)
  }
  const pin = (target: string, next: Partial<Declared>) =>
    change({
      declared: declared.map((entry) =>
        entry.name === target ? { ...entry, ...next } : entry,
      ),
    })
  const remove = (target: string) => {
    change({ declared: declared.filter((entry) => entry.name !== target) })
    setMenu(null)
    addButton.current?.focus()
  }
  const add = (entry: Declared) => {
    if (declared.some((other) => other.name === entry.name)) return
    change({ declared: [...declared, entry] })
    setQuery('')
  }
  const closePicker = () => {
    setPicker(false)
    setQuery('')
    addButton.current?.focus()
  }
  const pickTab = (next: Tab) => {
    setTab(next)
    rememberTab(next)
    setMenu(null)
    setIiiOpen(false)
    setPicker(false)
  }
  const requestClose = () => {
    if (saving) return
    if (dirty) setAsking(true)
    else onClose()
  }
  const discard = () => {
    setName(base.label)
    write(base.yaml, stack ? readOf(stack) : initial.read)
  }
  const save = async () => {
    if (!bridge || saving || status.blocked) return
    setSaving(true)
    setSaveError(null)
    try {
      if (mode === 'new') {
        onCreated?.(
          exact && copy
            ? await bridge.createStack(copy.id, name.trim())
            : await bridge.createStackFromYaml(name.trim(), yaml),
        )
      } else if (stack) {
        const next = await bridge.updateStack(stack.id, {
          label: name.trim(),
          yaml,
        })
        onSaved?.(next)
        setSaid(savedStatus(next))
      }
    } catch (cause) {
      setSaveError(errorText(cause))
    } finally {
      setSaving(false)
    }
  }
  const copyYaml = () => void copyText(yaml).then(setCopied)

  const showTemplates =
    !viewing &&
    ((mode === 'new' && source === 'template' && !template) || picking)
  const templatesAt =
    at.state === 'ready'
      ? `iii-hq/templates · ${at.read.ref} @ ${at.read.revision.slice(0, 8)}`
      : `iii-hq/templates · ${revisionKey}`
  const templatesSub =
    at.state === 'ready'
      ? plural(listed.length, 'project')
      : at.state === 'loading'
        ? 'reading…'
        : 'couldn’t be read'
  const tplLabel = template
    ? `From the ${shown.template?.id} template${tpl ? ` · ${tpl.workers.length}` : ''}`
    : ''
  const pins = new Map(
    declared.map((entry) => [
      packageName(entry.worker) ?? entry.name,
      pinLabel(entry),
    ]),
  )
  const verb = mode === 'new' ? 'created' : 'saved'
  const [tone, text] = saving
    ? ['faint', mode === 'new' ? 'Creating…' : 'Saving…']
    : viewing
      ? copyError
        ? ['alert', copyError]
        : ['faint', 'Read-only. Copies you make appear under This Console.']
      : saveError
        ? ['alert', `Not ${verb}. ${saveError}`]
        : mode === 'edit' && !dirty
          ? [
              'faint',
              said ||
                (stack?.updated_at
                  ? `Saved ${formatDateTime(stack.updated_at)}.`
                  : 'Saved.'),
            ]
          : [status.alert ? 'alert' : 'faint', status.text]
  const held = status.blocked || (mode === 'edit' && !dirty)
  const lines = blockedLines(yaml, blockedNames)
  const warnings = read?.warnings ?? []
  const fieldWarnings = [iiiWarning, minimum].filter(
    (warning): warning is string => Boolean(warning),
  )

  const title =
    mode === 'new'
      ? 'New stack'
      : mode === 'edit'
        ? `Edit ${stack?.label ?? ''}`
        : (stack?.label ?? '')
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) requestClose()
      }}
    >
      <DialogContent
        className="sk-sheet sb"
        data-narrow={narrow || undefined}
        data-stack-sheet={mode}
        data-tab={tab}
        onEscapeKeyDown={(event) => {
          // Escape closes what is open inside first: a menu, the picker.
          if (menu) {
            event.preventDefault()
            setMenu(null)
            menuButton.current?.focus()
          } else if (iiiOpen) {
            event.preventDefault()
            setIiiOpen(false)
            iiiButton.current?.focus()
          } else if (picker) {
            event.preventDefault()
            closePicker()
          }
        }}
      >
        <header className="sk-sheet-head sb-head">
          {viewing ? (
            <div className="sk-sheet-title-row">
              <DialogTitle className="sk-sheet-title">{title}</DialogTitle>
              <span className="sk-badge">
                <Lock size={14} aria-hidden="true" />
                Repository · read-only
              </span>
            </div>
          ) : (
            <DialogTitle
              className={mode === 'new' ? 'sb-title' : 'ds-visually-hidden'}
            >
              {title}
            </DialogTitle>
          )}
          <DialogDescription className="ds-visually-hidden">
            {viewing
              ? `${stack ? stackFile(stack) : ''}, built into this runner. Copy it to change a version, a worker or the template.`
              : 'The Form and the YAML tabs change the same draft. Only YAML that does not parse, or has no containers, is refused by the runner; what would not run as built holds the stack back here.'}
          </DialogDescription>
          <div className="sb-head-row">
            {viewing ? null : (
              <label className="sb-name">
                <span className="sk-field-label">Name</span>
                <input
                  id="sb-name"
                  className="sk-input sb-name-input"
                  value={name}
                  maxLength={160}
                  autoComplete="off"
                  readOnly={saving}
                  onChange={(event) => {
                    named.current = true
                    setSaveError(null)
                    setSaid('')
                    setName(event.target.value)
                  }}
                />
              </label>
            )}
            <SegmentedControl
              variant="tabs"
              aria-label="Change the stack in"
              className="sb-tabs"
              value={tab}
              onChange={pickTab}
              options={[
                { value: 'form', label: 'Form' },
                { value: 'yaml', label: 'YAML' },
              ]}
            />
          </div>
        </header>

        {tab === 'form' ? (
          <div
            className="sb-body"
            role="tabpanel"
            aria-label="Form"
            data-panel="form"
          >
            <fieldset className="sb-form" disabled={viewing || saving}>
              <legend className="ds-visually-hidden">The stack</legend>
              {gone ? <GoneAlert /> : null}
              {refused !== null ? (
                <div className="sk-alert sb-alert" role="alert">
                  <CircleX size={16} aria-hidden="true" />
                  <span>
                    <strong>The YAML doesn’t read.</strong> {refused} Fix it in
                    the YAML tab; the form waits for it.
                  </span>
                </div>
              ) : null}
              {warnings.length ? <Warnings warnings={warnings} /> : null}

              {mode === 'new' ? (
                <div className="sb-field">
                  <span className="sk-field-label" id="sb-from">
                    Start from
                  </span>
                  <div
                    className="sb-sources"
                    role="radiogroup"
                    aria-labelledby="sb-from"
                  >
                    {SOURCES.map((option) => (
                      <label
                        key={option.id}
                        className="sb-source"
                        data-selected={source === option.id || undefined}
                      >
                        <input
                          type="radio"
                          name="sb-source"
                          checked={source === option.id}
                          onChange={() => pickSource(option.id)}
                        />
                        <span className="sb-source-label">{option.label}</span>
                        <span className="sb-source-sub">
                          {option.id === 'none'
                            ? 'only the workers you declare'
                            : option.id === 'template'
                              ? templatesSub
                              : 'repository or this Console'}
                        </span>
                      </label>
                    ))}
                  </div>
                </div>
              ) : null}

              {mode === 'new' &&
              source === 'none' &&
              !template ? null : showTemplates ? (
                <fieldset className="sk-from sb-list">
                  <legend className="sb-legend">
                    <span className="sk-field-label">Template</span>
                    <span className="sb-meta">{templatesAt}</span>
                  </legend>
                  {at.state === 'loading' ? (
                    <p className="sk-hint" role="status">
                      Reading iii-hq/templates…
                    </p>
                  ) : null}
                  {at.state === 'failed' ? (
                    <div className="sk-alert sb-alert" role="alert">
                      <CircleX size={16} aria-hidden="true" />
                      <span>
                        <strong>iii-hq/templates couldn’t be read.</strong>{' '}
                        {at.message} Start with no template or from a copy
                        meanwhile.
                      </span>
                      <button
                        type="button"
                        className="sk-btn sk-btn-small sk-btn-fill"
                        onClick={() => loadTemplates(revisionKey)}
                      >
                        Try again
                      </button>
                    </div>
                  ) : null}
                  {mode === 'new' ? null : (
                    <label
                      className="sk-option sb-template"
                      data-selected={!template || undefined}
                    >
                      <input
                        type="radio"
                        className="sk-radio"
                        name="sb-template"
                        checked={!template}
                        aria-disabled={locks.template ? true : undefined}
                        onChange={() => {
                          if (!locks.template) pickTemplate(null)
                        }}
                      />
                      <span className="sk-option-text">
                        <span className="sk-option-label">No template</span>
                        <span className="sk-faint">
                          Each group starts from an empty project with the
                          workers declared here.
                        </span>
                      </span>
                    </label>
                  )}
                  {listed.map((entry) => {
                    const off = !entry.workers.length
                    const on = entry.id === shown.template?.id
                    const extra = off || on ? templateExtra(entry) : ''
                    return (
                      <label
                        key={entry.id}
                        className="sk-option sb-template"
                        data-selected={on || undefined}
                        data-off={off || undefined}
                        data-template={entry.id}
                      >
                        <input
                          type="radio"
                          className="sk-radio"
                          name="sb-template"
                          checked={on}
                          aria-disabled={
                            off || locks.template ? true : undefined
                          }
                          onChange={() => {
                            if (!off && !locks.template) pickTemplate(entry.id)
                          }}
                        />
                        <span className="sk-option-text">
                          <span className="sb-template-name">
                            <span className="sk-option-label">
                              {entry.name}
                            </span>
                            <span className="sb-meta">{entry.id}</span>
                          </span>
                          {entry.description ? (
                            <span className="sk-faint">
                              {entry.description}
                            </span>
                          ) : null}
                          {extra ? (
                            <span className="sk-faint">{extra}</span>
                          ) : null}
                        </span>
                        <span className="sb-meta sb-count">
                          {off
                            ? 'no workers'
                            : plural(entry.workers.length, 'worker')}
                        </span>
                      </label>
                    )
                  })}
                  <Locked id="sb-template-lock" reason={locks.template} />
                </fieldset>
              ) : (
                <div className="sb-field">
                  <span className="sk-field-label">Template</span>
                  <div className="sb-chosen">
                    <span className="sk-option-text">
                      <span className="sb-template-name">
                        <span className="sk-option-label">
                          {template
                            ? (tpl?.name ?? shown.template?.id)
                            : 'No template'}
                        </span>
                        {template ? (
                          <span className="sb-meta">{templatesAt}</span>
                        ) : null}
                      </span>
                      <span className="sk-faint">
                        {!template
                          ? 'Each group starts from an empty project with the workers declared here.'
                          : tpl
                            ? tpl.description
                            : at.state === 'loading'
                              ? 'Reading its workers…'
                              : revisionError
                                ? 'Its workers can’t be read at this revision.'
                                : 'Its workers couldn’t be read, so what this stack pins isn’t checked.'}
                      </span>
                    </span>
                    {viewing || (mode === 'new' && source === 'none') ? null : (
                      <button
                        type="button"
                        className="sk-btn sk-btn-small"
                        aria-label={
                          template
                            ? `Change the template ${shown.template?.id}`
                            : 'Choose a template'
                        }
                        aria-disabled={locks.template ? true : undefined}
                        aria-describedby={
                          locks.template ? 'sb-template-lock' : undefined
                        }
                        onClick={() => {
                          if (!locks.template) setPicking(true)
                        }}
                      >
                        Change
                      </button>
                    )}
                  </div>
                  {template ? (
                    <div className="sb-revision">
                      <label className="sk-field-label" htmlFor="sb-revision">
                        Revision
                      </label>
                      <input
                        id="sb-revision"
                        className="sk-input sk-input-mono"
                        value={revision}
                        placeholder="main"
                        autoComplete="off"
                        spellCheck={false}
                        readOnly={Boolean(locks.template)}
                        aria-invalid={revisionError ? true : undefined}
                        aria-describedby="sb-revision-hint"
                        onChange={(event) => {
                          if (!locks.template && shown.template)
                            change({
                              template: templateRef({
                                id: shown.template.id,
                                revision: event.target.value.trim() || null,
                              }),
                            })
                        }}
                      />
                      <span
                        id="sb-revision-hint"
                        className="sk-hint"
                        data-tone={revisionError ? 'alert' : undefined}
                      >
                        {revisionError ??
                          (revision
                            ? `Fixed at ${revision}: the template's workers as that commit, tag or branch has them.`
                            : 'Follows main each time it runs. Type a commit or a tag to fix it.')}
                      </span>
                    </div>
                  ) : null}
                  <Locked id="sb-template-lock" reason={locks.template} />
                </div>
              )}

              {mode === 'new' && source === 'copy' ? (
                <fieldset className="sk-from sb-list">
                  <legend className="sb-legend">
                    <span className="sk-field-label">Copy of</span>
                  </legend>
                  {stacks.map((other) => (
                    <label
                      key={other.id}
                      className="sk-option"
                      data-selected={copy?.id === other.id || undefined}
                    >
                      <input
                        type="radio"
                        className="sk-radio"
                        name="sb-copy"
                        checked={copy?.id === other.id}
                        onChange={() => pickCopy(other)}
                      />
                      <span className="sk-option-text">
                        <span className="sb-template-name">
                          <span className="sk-option-label">{other.label}</span>
                          <span className="sb-meta">
                            {other.source === 'local'
                              ? other.id
                              : stackFile(other)}
                          </span>
                        </span>
                        <span className="sk-faint">{stackDeclares(other)}</span>
                      </span>
                    </label>
                  ))}
                </fieldset>
              ) : null}

              <div className="sb-field sb-iii" data-iii="">
                <span className="sk-field-label" id="sb-iii-label">
                  iii release
                </span>
                <span className="sb-pin" data-iii-menu="">
                  <button
                    ref={iiiButton}
                    type="button"
                    className="sk-btn sk-btn-small sk-btn-fill"
                    aria-labelledby="sb-iii-label sb-iii-value"
                    aria-haspopup="dialog"
                    aria-expanded={iiiOpen}
                    aria-disabled={locks.iii ? true : undefined}
                    aria-describedby={locks.iii ? 'sb-iii-lock' : undefined}
                    onClick={() => {
                      if (locks.iii) return
                      setMenu(null)
                      setIiiOpen(!iiiOpen)
                    }}
                  >
                    <span className="sk-mono" id="sb-iii-value">
                      {iiiLabel(shown.iii, known)}
                    </span>
                    <ChevronDown size={14} aria-hidden="true" />
                  </button>
                  {iiiOpen ? (
                    <IiiMenu
                      iii={shown.iii}
                      releases={releases}
                      bridge={bridge}
                      onReload={loadReleases}
                      onChecked={(version, checked) =>
                        setChecks((current) => ({
                          ...current,
                          [version]: checked,
                        }))
                      }
                      onPick={(version) => {
                        change({ iii: version })
                        setIiiOpen(false)
                        iiiButton.current?.focus()
                      }}
                    />
                  ) : null}
                </span>
                <span className="sk-hint">{iiiHint(shown.iii)}</span>
                {fieldWarnings.map((warning) => (
                  <span key={warning} className="sk-hint" data-tone="warn">
                    {warning}
                  </span>
                ))}
                <Locked id="sb-iii-lock" reason={locks.iii} />
              </div>

              <section className="sb-workers" aria-labelledby="sb-workers">
                <div className="sb-heading">
                  <h3 id="sb-workers">Workers</h3>
                  <span className="sk-hint">
                    {template
                      ? 'the template brings them; declare one here to pin its version'
                      : 'each group starts these and what they depend on'}
                  </span>
                </div>

                {template ? (
                  // biome-ignore lint/a11y/useSemanticElements: a labelled part of the section, not a form group
                  <div className="sb-group" role="group" aria-label={tplLabel}>
                    <span className="sb-eyebrow">{tplLabel}</span>
                    {tpl ? (
                      // biome-ignore lint/a11y/noRedundantRoles: Safari drops the list role under list-style none
                      <ul role="list" className="sb-chips">
                        {tpl.workers.map((worker) => {
                          const pinned = pins.get(
                            packageName(worker.worker) ?? worker.name,
                          )
                          return (
                            <li
                              key={worker.name}
                              data-pinned={pinned ? true : undefined}
                              title={
                                pinned
                                  ? `${worker.name} pinned below`
                                  : `${worker.name} at the template’s version`
                              }
                            >
                              <span>{worker.name}</span>
                              <span className="sb-chip-pin">
                                {pinned ??
                                  worker.version ??
                                  (packageName(worker.worker)
                                    ? 'latest'
                                    : 'path')}
                              </span>
                            </li>
                          )
                        })}
                      </ul>
                    ) : null}
                    <span className="sk-hint">
                      Each group runs the template’s project: these workers, its
                      agents and skills. Workers it doesn’t declare are left
                      out.
                    </span>
                  </div>
                ) : null}

                {/* biome-ignore lint/a11y/useSemanticElements: a labelled part of the section, not a form group */}
                <div
                  className="sb-group"
                  role="group"
                  aria-label={
                    template ? 'Pinned by this stack' : 'Declared by this stack'
                  }
                >
                  <span className="sb-eyebrow">
                    {template
                      ? 'Pinned by this stack'
                      : 'Declared by this stack'}
                  </span>
                  {declared.length ? (
                    // biome-ignore lint/a11y/noRedundantRoles: Safari drops the list role under list-style none
                    <ul role="list" className="sb-declared">
                      {declared.map((entry, index) => (
                        <DeclaredRow
                          key={entry.name}
                          entry={entry}
                          verdict={verdicts[index]}
                          open={menu === entry.name}
                          locked={
                            viewing
                              ? null
                              : (locks.pins[entry.name] ?? locks.containers)
                          }
                          removable={!viewing && !locks.containers}
                          onToggle={(button) => {
                            menuButton.current = button
                            setIiiOpen(false)
                            setMenu(menu === entry.name ? null : entry.name)
                          }}
                          onChange={(next) => pin(entry.name, next)}
                          onRemove={() => remove(entry.name)}
                          onRetry={check}
                        />
                      ))}
                    </ul>
                  ) : (
                    <p className="sb-empty">
                      {template
                        ? 'Nothing pinned: every template worker runs at the template’s version. Add one of its workers to pin it.'
                        : 'No workers yet. Add the application under test and what the measurement needs.'}
                    </p>
                  )}
                </div>

                {viewing ? null : (
                  <button
                    ref={addButton}
                    type="button"
                    className="sk-btn sb-add"
                    aria-expanded={picker}
                    aria-controls={picker ? 'sb-picker' : undefined}
                    aria-disabled={locks.containers ? true : undefined}
                    aria-describedby={
                      locks.containers ? 'sb-containers-lock' : undefined
                    }
                    onClick={() => {
                      if (locks.containers) return
                      setMenu(null)
                      setIiiOpen(false)
                      setPicker(true)
                    }}
                  >
                    <Plus size={16} aria-hidden="true" />
                    Add worker
                  </button>
                )}
                <Locked id="sb-containers-lock" reason={locks.containers} />

                {/* biome-ignore lint/a11y/useSemanticElements: a labelled part of the section, not a form group */}
                <div
                  className="sb-group"
                  role="group"
                  aria-label="Added when it runs"
                >
                  <span className="sb-eyebrow">Added when it runs</span>
                  <dl className="sb-added">
                    {addedWhenItRuns(template, declared, lookups).map(
                      (entry) => (
                        <div key={entry.name}>
                          <dt>{entry.name}</dt>
                          <dd>{entry.why}</dd>
                        </div>
                      ),
                    )}
                  </dl>
                </div>
              </section>

              <details className="sb-timeouts">
                <summary>
                  Timeouts{' '}
                  <span className="sk-faint">
                    · startup_timeout and stop_timeout
                  </span>
                </summary>
                <p className="sk-hint">
                  How long a group waits for its workers to start and to stop.
                  Change them in the YAML tab.
                </p>
              </details>
            </fieldset>

            {picker ? (
              <aside
                id="sb-picker"
                className="sb-side"
                aria-label="Add a worker"
                data-picker=""
              >
                <div className="sb-side-bar">
                  <span className="sb-side-title">Add a worker</span>
                  <span className="sk-faint">
                    {template
                      ? `to the ${shown.template?.id} stack`
                      : 'to this stack'}
                  </span>
                  <button
                    type="button"
                    className="sk-btn sk-btn-small sb-push"
                    aria-label="Done, back to the YAML"
                    onClick={closePicker}
                  >
                    Done
                  </button>
                </div>
                <div className="sb-search">
                  <Search size={16} aria-hidden="true" />
                  <input
                    className="sk-input"
                    type="search"
                    aria-label="Search workers"
                    placeholder="Search, or type package://name"
                    value={query}
                    autoComplete="off"
                    spellCheck={false}
                    autoFocus
                    onChange={(event) => setQuery(event.target.value)}
                  />
                </div>
                <div className="sb-picks">
                  {groups.length ? (
                    groups.map((group) => (
                      // biome-ignore lint/a11y/useSemanticElements: a labelled list of choices, not a form group
                      <div
                        key={group.label}
                        className="sb-pick-group"
                        role="group"
                        aria-label={group.label}
                      >
                        <div className="sb-pick-head">
                          <span className="sb-eyebrow">{group.label}</span>
                          <span className="sk-faint">{group.sub}</span>
                          {group.retry ? (
                            <button
                              type="button"
                              className="sk-btn sk-btn-small sb-push"
                              onClick={() => group.retry && check(group.retry)}
                            >
                              Try again
                            </button>
                          ) : null}
                        </div>
                        {group.items.map((item) => (
                          <button
                            key={item.name}
                            type="button"
                            className="sb-pick"
                            aria-disabled={item.off || undefined}
                            data-pick={item.name}
                            onClick={() => {
                              if (item.add) add(item.add)
                              else if (!item.off) setShowAll(true)
                            }}
                          >
                            <span className="sb-pick-name">
                              <span className="sk-mono">{item.name}</span>
                              <span className="sb-pick-sub">{item.sub}</span>
                            </span>
                            <span
                              className="sb-pick-tag"
                              data-alert={item.alert || undefined}
                            >
                              {item.tag}
                            </span>
                          </button>
                        ))}
                      </div>
                    ))
                  ) : (
                    <p className="sb-empty">
                      Nothing matches. Type a whole package name to check the
                      registry.
                    </p>
                  )}
                </div>
                <p className="sb-side-foot">
                  A name that isn’t listed is checked against the iii registry
                  as you type, with what it depends on.
                </p>
              </aside>
            ) : (
              <aside className="sb-side" aria-label="YAML">
                <div className="sb-side-bar">
                  <span className="sk-yaml-file">YAML</span>
                  <span className="sk-faint">
                    {exact && copy
                      ? `copied as written from ${copy.source === 'local' ? copy.id : stackFile(copy)}`
                      : viewing && stack
                        ? stackFile(stack)
                        : 'the draft, as the YAML tab has it'}
                  </span>
                  <button
                    type="button"
                    className="sk-btn sk-btn-small sb-push"
                    onClick={copyYaml}
                  >
                    <Copy size={14} aria-hidden="true" />
                    {copied ? 'Copied' : 'Copy'}
                  </button>
                </div>
                <YamlCode
                  yaml={yaml}
                  marks={lines}
                  label={
                    exact
                      ? 'Stack YAML, copied as written'
                      : 'Stack YAML, read-only here'
                  }
                />
                <p className="sb-side-foot">
                  {template
                    ? 'With a template, containers only pin versions of the template’s workers.'
                    : 'Every container here starts in each group, with what it depends on.'}
                </p>
              </aside>
            )}
          </div>
        ) : (
          <div
            className="sk-sheet-body sb-yaml-body"
            role="tabpanel"
            aria-label="YAML"
            data-panel="yaml"
          >
            <div className="sk-yaml">
              <div className="sk-yaml-bar">
                <span className="sk-yaml-file">
                  {viewing && stack ? stackFile(stack) : 'YAML'}
                </span>
                <span className="sk-faint">
                  {viewing
                    ? yamlMeta(yaml)
                    : 'kept exactly as written, comments included'}
                </span>
                <button
                  type="button"
                  className="sk-btn sk-btn-small"
                  onClick={copyYaml}
                >
                  <Copy size={14} aria-hidden="true" />
                  {copied ? 'Copied' : 'Copy YAML'}
                </button>
              </div>
              {blocks.length && refused === null && !viewing ? (
                <div className="sb-blocks" role="status" id="sb-blocks">
                  <CircleX size={16} aria-hidden="true" />
                  <span>
                    <strong>
                      {mode === 'new'
                        ? 'Create waits for this:'
                        : 'Save waits for this:'}
                    </strong>
                    {/* biome-ignore lint/a11y/noRedundantRoles: Safari drops the list role under list-style none */}
                    <ul role="list">
                      {blocks.map((block) => (
                        <li key={block.text}>{block.text}</li>
                      ))}
                    </ul>
                  </span>
                </div>
              ) : null}
              {viewing ? (
                <YamlCode
                  yaml={yaml}
                  marks={lines}
                  label="Stack YAML, read-only"
                  keys
                />
              ) : (
                <div className="sk-editor">
                  <div className="sk-gutter" aria-hidden="true">
                    {yaml.split('\n').map((_, index) => (
                      <span
                        // biome-ignore lint/suspicious/noArrayIndexKey: line numbers
                        key={index}
                        data-blocked={lines[index] || undefined}
                      >
                        {index + 1}
                      </span>
                    ))}
                  </div>
                  <textarea
                    id="sb-yaml"
                    className="sk-textarea"
                    aria-label="Stack YAML"
                    aria-invalid={refused !== null ? true : undefined}
                    aria-describedby={
                      refused !== null
                        ? 'sb-refused'
                        : blocks.length
                          ? 'sb-blocks'
                          : undefined
                    }
                    spellCheck={false}
                    autoCapitalize="off"
                    autoCorrect="off"
                    wrap="off"
                    value={yaml}
                    readOnly={saving}
                    style={
                      {
                        '--sk-lines': yaml.split('\n').length,
                      } as CSSProperties
                    }
                    onChange={(event) => write(event.target.value, null)}
                  />
                </div>
              )}
            </div>
            <aside className="sk-aside" aria-label="What the runner reads">
              {gone ? <GoneAlert /> : null}
              {refused !== null ? (
                <div className="sk-alert" role="alert" id="sb-refused">
                  <CircleX size={16} aria-hidden="true" />
                  <span>
                    <strong>The runner refuses it.</strong> {refused}
                  </span>
                </div>
              ) : null}
              {warnings.length || fieldWarnings.length ? (
                <Warnings warnings={[...fieldWarnings, ...warnings]} />
              ) : refused === null && !pending ? (
                <p className="sk-faint">
                  No warnings. The runner reads the YAML as you type; only YAML
                  that doesn’t parse, or has no containers, is refused.
                </p>
              ) : null}
              {pending ? (
                <p className="sk-faint" role="status">
                  Reading the YAML…
                </p>
              ) : null}
            </aside>
          </div>
        )}

        <footer className="sk-sheet-foot">
          <span
            className="sk-status sb-status"
            role={saveError || (viewing && copyError) ? 'alert' : 'status'}
            data-tone={tone}
          >
            {tone === 'alert' ? <CircleX size={16} aria-hidden="true" /> : null}
            <span id="sb-status">{text}</span>
          </span>
          {viewing ? (
            <>
              <button type="button" className="sk-btn" onClick={onClose}>
                Close
              </button>
              <button
                type="button"
                className="sk-btn sk-btn-primary"
                disabled={!bridge}
                aria-disabled={busy || undefined}
                aria-busy={busy || undefined}
                onClick={() => {
                  if (!busy) onCopy?.()
                }}
              >
                {busy ? 'Copying…' : 'Copy to edit'}
              </button>
            </>
          ) : (
            <>
              {mode === 'edit' ? (
                <button
                  type="button"
                  className="sk-btn sb-delete"
                  aria-disabled={saving || undefined}
                  onClick={() => {
                    if (!saving) onDelete?.()
                  }}
                >
                  Delete
                </button>
              ) : null}
              <button
                type="button"
                className="sk-btn"
                aria-disabled={saving || undefined}
                onClick={() => {
                  if (saving) return
                  if (mode === 'edit' && dirty) discard()
                  else requestClose()
                }}
              >
                {mode === 'new'
                  ? 'Cancel'
                  : dirty
                    ? 'Discard changes'
                    : 'Close'}
              </button>
              <button
                type="button"
                className="sk-btn sk-btn-primary"
                disabled={!bridge}
                aria-disabled={held || saving || undefined}
                aria-busy={saving || undefined}
                aria-describedby="sb-status"
                onClick={() => {
                  if (!held) void save()
                }}
              >
                {saving
                  ? mode === 'new'
                    ? 'Creating…'
                    : 'Saving…'
                  : mode === 'new'
                    ? 'Create stack'
                    : 'Save stack'}
              </button>
            </>
          )}
        </footer>
      </DialogContent>
      <ConfirmDialog
        open={asking}
        onOpenChange={(open) => {
          if (!open) setAsking(false)
        }}
        title={
          mode === 'new'
            ? 'Discard this new stack?'
            : `Discard changes to ${stack?.label ?? 'this stack'}?`
        }
        description={
          mode === 'new'
            ? 'What you picked and typed here is not saved. Discarding closes New stack without creating it.'
            : 'What you changed in it is not saved. Discarding keeps the stack as it was last saved.'
        }
        cancelLabel="Keep editing"
        confirmLabel="Discard changes"
        tone="danger"
        onConfirm={onClose}
      />
    </Dialog>
  )
}

function GoneAlert() {
  return (
    <div className="sk-alert" role="alert" data-stack-gone>
      <CircleX size={16} aria-hidden="true" />
      <span>
        This stack is no longer in this Console. Copy the YAML before closing.
      </span>
    </div>
  )
}

function Warnings({ warnings }: { warnings: string[] }) {
  return (
    <div className="sk-warnbox" role="status" data-stack-warnings>
      <span className="sk-warnbox-title">
        <TriangleAlert size={16} aria-hidden="true" />
        {warningsTitle(warnings.length)}
      </span>
      <ul>
        {warnings.map((warning, index) => (
          // biome-ignore lint/suspicious/noArrayIndexKey: warnings repeat and never reorder
          <li key={index}>{warning}</li>
        ))}
      </ul>
      <span className="sk-warnbox-note">Warnings never stop a run.</span>
    </div>
  )
}

/** Why a field is read-only here, when it is. */
function Locked({ id, reason }: { id: string; reason: string | null }) {
  if (!reason || reason === 'Read-only.') return null
  return (
    <span id={id} className="sk-hint" data-tone="warn" data-locked="">
      <Lock size={12} aria-hidden="true" /> {reason}
    </span>
  )
}

/** The draft's lines, read-only, each container that holds the stack back
 *  marked. */
function YamlCode({
  yaml,
  marks,
  label,
  keys = false,
}: {
  yaml: string
  marks: boolean[]
  label: string
  /** Draw a mapping key apart from its value, comments faint. */
  keys?: boolean
}) {
  return (
    <section
      className="sk-code"
      aria-label={label}
      // biome-ignore lint/a11y/noNoninteractiveTabindex: a scrolling region reads by keyboard
      tabIndex={0}
    >
      {yaml
        .replace(/\n$/, '')
        .split('\n')
        .map((line, index) => {
          const part = keys ? yamlLine(line) : null
          return (
            <div
              // biome-ignore lint/suspicious/noArrayIndexKey: the lines of the draft
              key={index}
              className="sk-line"
              data-blocked={marks[index] || undefined}
            >
              <span className="sk-ln" aria-hidden="true">
                {index + 1}
              </span>
              {part ? (
                <span
                  className="sk-text"
                  data-comment={part.comment || undefined}
                >
                  {part.key ? <span className="sk-key">{part.key}</span> : null}
                  {part.rest}
                </span>
              ) : (
                <span className="sk-text">{line || ' '}</span>
              )}
            </div>
          )
        })}
    </section>
  )
}

/** The iii release: the newest candidate (`latest`), a recent release, or
 *  another version, checked before it is used. */
function IiiMenu({
  iii,
  releases,
  bridge,
  onReload,
  onChecked,
  onPick,
}: {
  iii: string | null
  releases:
    | { state: 'loading' }
    | { state: 'ready'; read: IiiReleases }
    | { state: 'failed'; message: string }
  bridge: DashboardDataBridge | null
  onReload: () => void
  onChecked: (version: string, checked: IiiReleases['checked']) => void
  onPick: (version: string) => void
}) {
  const known = releases.state === 'ready' ? releases.read : null
  const choice = iiiChoice(iii, known)
  const [other, setOther] = useState(choice === 'other')
  const [typed, setTyped] = useState(
    choice === 'other' && iii !== null ? iii : '',
  )
  const [checking, setChecking] = useState<{
    state: 'checking' | 'refused'
    message?: string
  } | null>(null)
  const now = new Date()
  const candidate = known?.releases.find(
    (release) => release.version === known.latest_candidate,
  )
  const confirm = () => {
    const version = typed.trim()
    if (!bridge || !version || checking?.state === 'checking') return
    setChecking({ state: 'checking' })
    bridge.listIiiReleases(version).then(
      (answer) => {
        onChecked(version, answer.checked)
        const refusal = iiiRefusal(answer.checked)
        if (refusal) setChecking({ state: 'refused', message: refusal })
        else {
          setChecking(null)
          onPick(version)
        }
      },
      (cause) => setChecking({ state: 'refused', message: errorText(cause) }),
    )
  }
  return (
    <div
      className="sb-menu sb-iii-menu"
      role="dialog"
      aria-label="iii release"
      data-iii-menu=""
    >
      {releases.state === 'loading' ? (
        <p className="sk-hint" role="status">
          Reading iii-hq/iii’s releases…
        </p>
      ) : null}
      {releases.state === 'failed' ? (
        <div className="sk-alert sb-alert" role="alert">
          <CircleX size={16} aria-hidden="true" />
          <span>
            <strong>iii-hq/iii couldn’t be read.</strong> {releases.message}
          </span>
          <button
            type="button"
            className="sk-btn sk-btn-small sk-btn-fill"
            onClick={onReload}
          >
            Try again
          </button>
        </div>
      ) : null}
      <label
        className="sk-option"
        data-selected={(!other && choice === 'latest') || undefined}
        data-iii-option="latest"
      >
        <input
          type="radio"
          className="sk-radio"
          name="sb-iii"
          checked={!other && choice === 'latest'}
          onChange={() => onPick('latest')}
        />
        <span className="sk-option-text">
          <span className="sb-option-label">
            Latest release candidate
            {known?.latest_candidate
              ? ` · ${known.latest_candidate}${candidate ? ` ${releasedOn(candidate, now)}` : ''}`
              : ''}
          </span>
          <span className="sk-faint">
            The newest X.Y.Z-rc.N, resolved when an execution starts.
          </span>
        </span>
      </label>
      {known?.releases.length ? (
        // biome-ignore lint/a11y/useSemanticElements: a labelled list of choices inside the radio group
        <div
          className="sb-iii-releases"
          role="group"
          aria-label="Recent releases"
        >
          <span className="sb-eyebrow">Recent releases</span>
          {known.releases.map((release) => {
            const on = !other && choice === `release:${release.version}`
            return (
              <label
                key={release.version}
                className="sk-option"
                data-selected={on || undefined}
                data-off={!release.cli || undefined}
                data-iii-option={release.version}
              >
                <input
                  type="radio"
                  className="sk-radio"
                  name="sb-iii"
                  checked={on}
                  aria-disabled={!release.cli || undefined}
                  onChange={() => {
                    if (release.cli) onPick(release.version)
                  }}
                />
                <span className="sk-option-text">
                  <span className="sb-template-name">
                    <span className="sk-mono">{release.version}</span>
                    <span className="sb-meta">
                      {release.prerelease ? 'release candidate' : 'stable'}
                      {release.published_at
                        ? ` · ${releasedOn(release, now)}`
                        : ''}
                      {release.cli ? '' : ' · no CLI for x86_64 Linux'}
                    </span>
                  </span>
                </span>
              </label>
            )
          })}
        </div>
      ) : null}
      <label
        className="sk-option"
        data-selected={other || choice === 'other' || undefined}
        data-iii-option="other"
      >
        <input
          type="radio"
          className="sk-radio"
          name="sb-iii"
          checked={other || choice === 'other'}
          onChange={() => setOther(true)}
        />
        <span className="sk-option-text">
          <span className="sb-option-label">Another version…</span>
          <span className="sk-faint">
            Any iii/v tag of iii-hq/iii that publishes the CLI; checked before
            it is used.
          </span>
          {other || choice === 'other' ? (
            <span className="sb-iii-other">
              <input
                className="sk-input sk-input-mono sb-menu-input"
                aria-label="iii version"
                value={typed}
                placeholder="e.g. 0.24.2"
                autoComplete="off"
                spellCheck={false}
                aria-invalid={checking?.state === 'refused' || undefined}
                aria-describedby="sb-iii-check"
                onChange={(event) => {
                  setTyped(event.target.value)
                  setChecking(null)
                }}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') {
                    event.preventDefault()
                    confirm()
                  }
                }}
              />
              <button
                type="button"
                className="sk-btn sk-btn-small"
                aria-disabled={
                  !typed.trim() || checking?.state === 'checking' || undefined
                }
                onClick={confirm}
              >
                {checking?.state === 'checking' ? 'Checking…' : 'Use'}
              </button>
            </span>
          ) : null}
          {checking ? (
            <span
              id="sb-iii-check"
              className="sk-hint"
              role={checking.state === 'refused' ? 'alert' : 'status'}
              data-tone={checking.state === 'refused' ? 'alert' : undefined}
            >
              {checking.state === 'checking'
                ? `Checking iii/v${typed.trim()}…`
                : checking.message}
            </span>
          ) : null}
        </span>
      </label>
      <p className="sb-menu-note">Changes the YAML as you pick.</p>
    </div>
  )
}

/** One declared worker's version: latest, a version, or (package:// only) a
 *  commit, each written into the YAML as it is picked. */
function VersionMenu({
  entry,
  onChange,
}: {
  entry: Declared
  onChange: (change: Partial<Declared>) => void
}) {
  const buildable = Boolean(packageName(entry.worker))
  const mode =
    entry.commit !== null
      ? 'commit'
      : entry.version !== 'latest'
        ? 'version'
        : 'latest'
  const group = `sb-pin-${entry.name}`
  const options = [
    {
      id: 'latest',
      label: 'Latest',
      sub: 'The newest release, resolved when an execution starts.',
      pick: () => onChange({ version: 'latest', commit: null }),
    },
    {
      id: 'version',
      label: 'A version',
      sub: `A release of ${entry.name} in the iii registry.`,
      pick: () => onChange({ version: '', commit: null }),
      input: {
        value: mode === 'version' ? entry.version : '',
        placeholder: 'e.g. 1.4.2',
        change: (value: string) => onChange({ version: value, commit: null }),
      },
    },
    {
      id: 'commit',
      label: 'A commit',
      sub: buildable
        ? 'Built from source at this commit, with the dependencies it declares.'
        : 'Only a package:// worker is built from a commit.',
      off: !buildable,
      pick: () => onChange({ commit: '' }),
      input: buildable
        ? {
            value: entry.commit ?? '',
            placeholder: 'a commit of the worker’s repository',
            change: (value: string) => onChange({ commit: value }),
          }
        : undefined,
    },
  ]
  return (
    <div
      className="sb-menu"
      role="dialog"
      aria-label={`Version of ${entry.name}`}
      data-version-menu={entry.name}
    >
      {options.map((option) => {
        const on = mode === option.id
        return (
          <label
            key={option.id}
            className="sk-option"
            data-selected={on || undefined}
            data-off={option.off || undefined}
          >
            <input
              type="radio"
              className="sk-radio"
              name={group}
              checked={on}
              aria-disabled={option.off || undefined}
              onChange={() => {
                if (!option.off && !on) option.pick()
              }}
            />
            <span className="sk-option-text">
              <span className="sb-option-label">{option.label}</span>
              <span className="sk-faint">{option.sub}</span>
              {option.input && on ? (
                <input
                  className="sk-input sk-input-mono sb-menu-input"
                  aria-label={option.label}
                  value={option.input.value}
                  placeholder={option.input.placeholder}
                  autoComplete="off"
                  spellCheck={false}
                  onChange={(event) => option.input?.change(event.target.value)}
                />
              ) : null}
            </span>
          </label>
        )
      })}
      <p className="sb-menu-note">Changes the YAML as you pick.</p>
    </div>
  )
}

/** A declared worker: its name and reference, its version menu, Remove, and
 *  what it does; read-only, with why, where the form can't patch it. */
function DeclaredRow({
  entry,
  verdict,
  open,
  locked,
  removable,
  onToggle,
  onChange,
  onRemove,
  onRetry,
}: {
  entry: Declared
  verdict: Verdict | null
  open: boolean
  locked: string | null
  removable: boolean
  onToggle: (button: HTMLButtonElement) => void
  onChange: (change: Partial<Declared>) => void
  onRemove: () => void
  onRetry: (worker: string) => void
}) {
  const lockId = `sb-lock-${entry.name}`
  return (
    <li data-worker={entry.name} data-tone={verdict?.tone}>
      <div className="sb-row">
        <span className="sb-row-name">
          <span className="sk-mono">{entry.name}</span>
          <span className="sb-ref">{entry.worker}</span>
        </span>
        <span className="sb-pin" data-pin={entry.name}>
          <button
            type="button"
            className="sk-btn sk-btn-small sk-btn-fill"
            aria-label={`Version of ${entry.name}`}
            aria-haspopup="dialog"
            aria-expanded={open}
            aria-disabled={locked ? true : undefined}
            aria-describedby={locked ? lockId : undefined}
            onClick={(event) => {
              if (!locked) onToggle(event.currentTarget)
            }}
          >
            <span className="sk-mono">{pinLabel(entry)}</span>
            <ChevronDown size={14} aria-hidden="true" />
          </button>
          {open && !locked ? (
            <VersionMenu entry={entry} onChange={onChange} />
          ) : null}
        </span>
        {removable ? (
          <button
            type="button"
            className="sk-btn sk-btn-small sk-btn-icon"
            aria-label={`Remove ${entry.name}`}
            title="Remove"
            onClick={onRemove}
          >
            <X size={16} aria-hidden="true" />
          </button>
        ) : null}
      </div>
      {verdict ? (
        <p className="sb-note" data-tone={verdict.tone}>
          <span className="sb-dot" aria-hidden="true" />
          <span>
            <strong>{verdict.lead}</strong>
            {verdict.text ? ` ${verdict.text}` : ''}
          </span>
          {verdict.retry ? (
            <button
              type="button"
              className="sk-btn sk-btn-small sb-retry"
              onClick={() => verdict.retry && onRetry(verdict.retry)}
            >
              Try again
            </button>
          ) : null}
        </p>
      ) : null}
      <Locked id={lockId} reason={locked} />
    </li>
  )
}
