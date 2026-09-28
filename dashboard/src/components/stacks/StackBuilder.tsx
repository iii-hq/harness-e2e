import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from '@iii-dev/console-ui'
import { ChevronDown, CircleX, Copy, Plus, Search, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { copyText } from '@/lib/clipboard'
import type {
  DashboardDataBridge,
  Stack,
  StackContainer,
  StackTemplates,
} from '@/lib/dashboard-data-source'
import { plural } from '@/lib/format'
import {
  addedWhenItRuns,
  builderStatus,
  type Declared,
  declaredOf,
  judge,
  type Lookups,
  lookupOf,
  markedLines,
  orderTemplates,
  packageName,
  patchYaml,
  pickerGroups,
  pinLabel,
  type Source,
  sameDeclared,
  templateExtra,
  templateId,
  typedName,
  type Verdict,
  yamlOf,
} from '@/lib/stack-builder'
import { iiiHint, stackDeclares, stackFile } from '@/lib/stacks-view'
import './stack-builder.css'

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

/** What the builder hands the YAML editor: a stack not saved yet, and
 *  what to say about it. */
export type StackDraft = Pick<
  Stack,
  'label' | 'yaml' | 'iii' | 'template' | 'containers'
> & { notice?: string }

type Templates =
  | { state: 'loading' }
  | { state: 'ready'; read: StackTemplates }
  | { state: 'failed'; message: string }

const SOURCES: Array<{ id: Source; label: string }> = [
  { id: 'none', label: 'No template' },
  { id: 'template', label: 'An iii-hq/templates project' },
  { id: 'copy', label: 'Copy of a stack' },
]

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
      <p className="sb-menu-note">Changes the YAML on the right as you pick.</p>
    </div>
  )
}

/** New stack as the canvas draws it (StackBuilder.dc.html): a name, where it
 *  starts from (no template, an iii-hq/templates project or a copy), the
 *  workers it declares or pins with what each does, a picker, what the
 *  executor adds, and the YAML written from the form. Create is held back,
 *  with the reason, while the stack would not run as built; Edit as YAML
 *  hands what is written to the editor instead. */
export function StackBuilder({
  stacks,
  narrow,
  bridge,
  onCreated,
  onEditYaml,
  onClose,
}: {
  stacks: Stack[]
  narrow: boolean
  bridge: DashboardDataBridge
  onCreated: (stack: Stack) => void
  onEditYaml: (draft: StackDraft) => void
  onClose: () => void
}) {
  const [name, setName] = useState('')
  const named = useRef(false)
  const [source, setSource] = useState<Source>('template')
  const [template, setTemplate] = useState<string | null>(null)
  const [picking, setPicking] = useState(true)
  const [iii, setIii] = useState('latest')
  const [copy, setCopy] = useState<Stack | null>(null)
  const [declared, setDeclared] = useState<Declared[]>([])
  const [menu, setMenu] = useState<string | null>(null)
  const [picker, setPicker] = useState(false)
  const [query, setQuery] = useState('')
  const [showAll, setShowAll] = useState(false)
  const [templates, setTemplates] = useState<Templates>({ state: 'loading' })
  const [lookups, setLookups] = useState<Lookups>({})
  const [creating, setCreating] = useState(false)
  const [createError, setCreateError] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)
  const menuButton = useRef<HTMLButtonElement | null>(null)
  const addButton = useRef<HTMLButtonElement>(null)

  const loadTemplates = useCallback(() => {
    setTemplates({ state: 'loading' })
    bridge.listStackTemplates().then(
      (read) => setTemplates({ state: 'ready', read }),
      (cause) => setTemplates({ state: 'failed', message: errorText(cause) }),
    )
  }, [bridge])
  useEffect(() => {
    loadTemplates()
  }, [loadTemplates])

  // Each name is asked once while it is answered or being asked; one the
  // registry did not answer about warns, blocks nothing and can be asked
  // again with Try again.
  const asked = useRef(new Set<string>())
  const check = useCallback(
    (worker: string) => {
      if (asked.current.has(worker)) return
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
  // Without a template, what each declared worker brings decides what may
  // be declared beside it.
  useEffect(() => {
    if (template) return
    for (const entry of declared) {
      const own = packageName(entry.worker)
      if (own) check(own)
    }
  }, [template, declared, check])

  const read = templates.state === 'ready' ? templates.read : null
  const listed = useMemo(() => orderTemplates(read?.templates ?? []), [read])
  const tpl = listed.find((entry) => entry.id === templateId(template)) ?? null
  const revision = read ? `main @ ${read.revision.slice(0, 8)}` : 'main'
  const verdicts = declared.map((entry) =>
    judge(entry, template, tpl, declared, lookups),
  )
  const status = builderStatus({
    name,
    source,
    template,
    tpl,
    declared,
    verdicts,
  })
  // A copy the form has not touched is created as written, comments and
  // every key; once touched, the form writes each worker and its pin.
  const copyOf = source === 'copy' ? copy : null
  const exact =
    copyOf !== null &&
    template === copyOf.template &&
    sameDeclared(declared, declaredOf(copyOf))
  const lines = exact
    ? copyOf.yaml.replace(/\n$/, '').split('\n')
    : yamlOf(iii, template, declared)
  const yaml = exact ? copyOf.yaml : `${lines.join('\n')}\n`
  const blockedNames = new Set(
    declared
      .filter((_, index) => verdicts[index]?.tone === 'block')
      .map((entry) => entry.name),
  )
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
  // A whole name no list holds is checked as it is typed.
  const typed = picker ? typedName(query, groups) : null
  useEffect(() => {
    if (!typed) return
    const timer = setTimeout(() => check(typed), 300)
    return () => clearTimeout(timer)
  }, [typed, check])

  // An open version menu closes on a click outside it.
  useEffect(() => {
    if (!menu) return
    const outside = (event: PointerEvent) => {
      const target = event.target as Element | null
      if (
        target?.closest?.(`[data-version-menu="${menu}"]`) ||
        target?.closest?.(`[data-pin="${menu}"]`)
      )
        return
      setMenu(null)
    }
    document.addEventListener('pointerdown', outside, true)
    return () => document.removeEventListener('pointerdown', outside, true)
  }, [menu])

  const nameAfter = (next: string) => {
    if (!named.current) setName(next)
  }
  const edit = (change: () => void) => {
    setCreateError(null)
    setCopied(false)
    change()
  }
  const pickSource = (next: Source) =>
    edit(() => {
      setSource(next)
      setMenu(null)
      if (next === 'none') {
        setTemplate(null)
        setPicking(false)
      }
      if (next === 'template') setPicking(true)
    })
  const pickTemplate = (id: string) =>
    edit(() => {
      setTemplate(id)
      setPicking(false)
      setShowAll(false)
      nameAfter(id)
    })
  const pickCopy = (stack: Stack) =>
    edit(() => {
      setCopy(stack)
      setTemplate(stack.template)
      setIii(stack.iii ?? 'latest')
      setDeclared(declaredOf(stack))
      setMenu(null)
      nameAfter(`${stack.label} · copy`)
    })
  const patch = (target: string, change: Partial<Declared>) =>
    edit(() =>
      setDeclared((current) =>
        current.map((entry) =>
          entry.name === target ? { ...entry, ...change } : entry,
        ),
      ),
    )
  const remove = (target: string) =>
    edit(() => {
      setDeclared((current) => current.filter((entry) => entry.name !== target))
      setMenu(null)
      addButton.current?.focus()
    })
  const add = (entry: Declared) =>
    edit(() => {
      setDeclared((current) =>
        current.some((other) => other.name === entry.name)
          ? current
          : [...current, entry],
      )
      setQuery('')
    })
  const closePicker = () => {
    setPicker(false)
    setQuery('')
    addButton.current?.focus()
  }
  const create = async () => {
    if (status.blocked || creating) return
    setCreating(true)
    setCreateError(null)
    try {
      onCreated(
        exact
          ? await bridge.createStack(copyOf.id, name.trim())
          : await bridge.createStackFromYaml(name.trim(), yaml),
      )
    } catch (cause) {
      setCreateError(errorText(cause))
    } finally {
      setCreating(false)
    }
  }
  // A touched copy goes to the editor as its own YAML with the form's
  // changes made to those containers' lines, so nothing else is lost.
  const patched =
    copyOf && !exact
      ? patchYaml(
          copyOf.yaml,
          declaredOf(copyOf),
          declared,
          template,
          copyOf.template,
        )
      : null
  const editYaml = () =>
    onEditYaml({
      label: name.trim() || 'New stack',
      yaml: copyOf && !exact ? (patched ?? copyOf.yaml) : yaml,
      notice:
        copyOf && !exact && patched === null
          ? `This is ${copyOf.label} as written, without the form's changes: its YAML isn't laid out as the form writes one (containers two spaces in, their keys four). Make them here.`
          : undefined,
      iii,
      template,
      containers: declared.map(
        (entry): StackContainer => ({
          name: entry.name,
          worker: entry.worker,
          version: entry.commit === null ? entry.version || 'latest' : null,
          commit: entry.commit,
        }),
      ),
    })

  const showTemplates = source === 'template' && (picking || !template)
  const showChosen = Boolean(template) && !showTemplates
  const templatesSub =
    templates.state === 'ready'
      ? plural(listed.length, 'project')
      : templates.state === 'loading'
        ? 'reading…'
        : 'couldn’t be read'
  const statusText = creating
    ? 'Creating…'
    : createError
      ? `Not created. ${createError}`
      : status.text
  const alert = Boolean(createError) || status.alert
  const tplLabel = template
    ? `From the ${templateId(template)} template${tpl ? ` · ${tpl.workers.length}` : ''}`
    : ''
  // What this stack pins of each template worker, by package.
  const pins = new Map(
    declared.map((entry) => [
      packageName(entry.worker) ?? entry.name,
      pinLabel(entry),
    ]),
  )

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !creating) onClose()
      }}
    >
      <DialogContent
        className="sk-sheet sb"
        data-narrow={narrow || undefined}
        data-stack-builder=""
        onEscapeKeyDown={(event) => {
          // Escape closes what is open inside first: the menu, the picker.
          if (menu) {
            event.preventDefault()
            setMenu(null)
            menuButton.current?.focus()
          } else if (picker) {
            event.preventDefault()
            closePicker()
          }
        }}
      >
        <header className="sk-sheet-head sb-head">
          <DialogTitle className="sb-title">New stack</DialogTitle>
          <DialogDescription className="ds-visually-hidden">
            Start from an iii-hq/templates project, a copy of a stack or no
            template, pick its workers and their versions; the YAML is written
            as you go.
          </DialogDescription>
          <label className="sb-name">
            <span className="sk-field-label">Name</span>
            <input
              id="sb-name"
              className="sk-input sb-name-input"
              value={name}
              maxLength={160}
              autoComplete="off"
              readOnly={creating}
              onChange={(event) =>
                edit(() => {
                  named.current = true
                  setName(event.target.value)
                })
              }
            />
          </label>
        </header>

        <div className="sb-body">
          <div className="sb-form">
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

            {showTemplates ? (
              <fieldset className="sk-from sb-list">
                <legend className="sb-legend">
                  <span className="sk-field-label">Template</span>
                  <span className="sb-meta">iii-hq/templates · {revision}</span>
                </legend>
                {templates.state === 'loading' ? (
                  <p className="sk-hint" role="status">
                    Reading iii-hq/templates…
                  </p>
                ) : null}
                {templates.state === 'failed' ? (
                  <div className="sk-alert sb-alert" role="alert">
                    <CircleX size={16} aria-hidden="true" />
                    <span>
                      <strong>iii-hq/templates couldn’t be read.</strong>{' '}
                      {templates.message} Start with no template or from a copy
                      meanwhile.
                    </span>
                    <button
                      type="button"
                      className="sk-btn sk-btn-small sk-btn-fill"
                      onClick={loadTemplates}
                    >
                      Try again
                    </button>
                  </div>
                ) : null}
                {listed.map((entry) => {
                  const off = !entry.workers.length
                  const on = entry.id === templateId(template)
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
                        aria-disabled={off || undefined}
                        onChange={() => {
                          if (!off) pickTemplate(entry.id)
                        }}
                      />
                      <span className="sk-option-text">
                        <span className="sb-template-name">
                          <span className="sk-option-label">{entry.name}</span>
                          <span className="sb-meta">{entry.id}</span>
                        </span>
                        {entry.description ? (
                          <span className="sk-faint">{entry.description}</span>
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
              </fieldset>
            ) : null}

            {showChosen ? (
              <div className="sb-field">
                <span className="sk-field-label">Template</span>
                <div className="sb-chosen">
                  <span className="sk-option-text">
                    <span className="sb-template-name">
                      <span className="sk-option-label">
                        {tpl?.name ?? templateId(template)}
                      </span>
                      <span className="sb-meta">
                        {template} · {revision}
                      </span>
                    </span>
                    <span className="sk-faint">
                      {tpl
                        ? tpl.description
                        : templates.state === 'loading'
                          ? 'Reading its workers…'
                          : 'Its workers couldn’t be read, so what this stack pins isn’t checked.'}
                    </span>
                  </span>
                  <button
                    type="button"
                    className="sk-btn sk-btn-small"
                    aria-label={`Change the template ${templateId(template)}`}
                    onClick={() =>
                      edit(() => {
                        setSource('template')
                        setPicking(true)
                      })
                    }
                  >
                    Change
                  </button>
                </div>
              </div>
            ) : null}

            {source === 'copy' ? (
              <fieldset className="sk-from sb-list">
                <legend className="sb-legend">
                  <span className="sk-field-label">Copy of</span>
                </legend>
                {stacks.map((stack) => (
                  <label
                    key={stack.id}
                    className="sk-option"
                    data-selected={copy?.id === stack.id || undefined}
                  >
                    <input
                      type="radio"
                      className="sk-radio"
                      name="sb-copy"
                      checked={copy?.id === stack.id}
                      onChange={() => pickCopy(stack)}
                    />
                    <span className="sk-option-text">
                      <span className="sb-template-name">
                        <span className="sk-option-label">{stack.label}</span>
                        <span className="sb-meta">
                          {stack.source === 'local'
                            ? stack.id
                            : stackFile(stack)}
                        </span>
                      </span>
                      <span className="sk-faint">{stackDeclares(stack)}</span>
                    </span>
                  </label>
                ))}
                {copyOf && !exact ? (
                  <p className="sk-hint sb-copy-note" data-tone="warn">
                    Creating from the form keeps each worker and its pin;
                    comments and other keys of {copyOf.label} are left out. Edit
                    as YAML keeps them.
                  </p>
                ) : null}
              </fieldset>
            ) : null}

            <div className="sb-iii">
              <span className="sk-field-label">iii release</span>
              <span className="sb-iii-value">{iii}</span>
              <span className="sk-hint">{iiiHint(iii)}</span>
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
                        const pin = pins.get(
                          packageName(worker.worker) ?? worker.name,
                        )
                        return (
                          <li
                            key={worker.name}
                            data-pinned={pin ? true : undefined}
                            title={
                              pin
                                ? `${worker.name} pinned below`
                                : `${worker.name} at the template’s version`
                            }
                          >
                            <span>{worker.name}</span>
                            <span className="sb-chip-pin">
                              {pin ??
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
                    agents and skills. Workers it doesn’t declare are left out.
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
                  {template ? 'Pinned by this stack' : 'Declared by this stack'}
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
                        onToggle={(button) => {
                          menuButton.current = button
                          setMenu(menu === entry.name ? null : entry.name)
                        }}
                        onChange={(change) => patch(entry.name, change)}
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

              <button
                ref={addButton}
                type="button"
                className="sk-btn sb-add"
                aria-expanded={picker}
                aria-controls={picker ? 'sb-picker' : undefined}
                onClick={() => {
                  setMenu(null)
                  setPicker(true)
                }}
              >
                <Plus size={16} aria-hidden="true" />
                Add worker
              </button>

              {/* biome-ignore lint/a11y/useSemanticElements: a labelled part of the section, not a form group */}
              <div
                className="sb-group"
                role="group"
                aria-label="Added when it runs"
              >
                <span className="sb-eyebrow">Added when it runs</span>
                <dl className="sb-added">
                  {addedWhenItRuns(template, declared, lookups).map((entry) => (
                    <div key={entry.name}>
                      <dt>{entry.name}</dt>
                      <dd>{entry.why}</dd>
                    </div>
                  ))}
                </dl>
              </div>
            </section>

            <details className="sb-timeouts">
              <summary>
                Timeouts{' '}
                <span className="sk-faint">
                  · starts within 5m · stops within 30s
                </span>
              </summary>
              <p className="sk-hint">
                Written as startup_timeout and stop_timeout. Change them with
                Edit as YAML.
              </p>
            </details>
          </div>

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
                    ? `to the ${templateId(template)} stack`
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
                A name that isn’t listed is checked against the iii registry as
                you type, with what it depends on.
              </p>
            </aside>
          ) : (
            <aside className="sb-side" aria-label="YAML">
              <div className="sb-side-bar">
                <span className="sk-yaml-file">YAML</span>
                <span className="sk-faint">
                  {exact
                    ? `copied as written from ${copyOf.source === 'local' ? copyOf.id : stackFile(copyOf)}`
                    : 'written from the form'}
                </span>
                <button
                  type="button"
                  className="sk-btn sk-btn-small sb-push"
                  onClick={() => void copyText(yaml).then(setCopied)}
                >
                  <Copy size={14} aria-hidden="true" />
                  {copied ? 'Copied' : 'Copy'}
                </button>
                <button
                  type="button"
                  className="sk-btn sk-btn-small sb-outline"
                  aria-disabled={creating || undefined}
                  onClick={() => {
                    if (!creating) editYaml()
                  }}
                >
                  Edit as YAML
                </button>
              </div>
              <section
                className="sk-code"
                aria-label={
                  exact
                    ? 'Stack YAML, copied as written'
                    : 'Stack YAML, written from the form'
                }
                // biome-ignore lint/a11y/noNoninteractiveTabindex: a scrolling region reads by keyboard
                tabIndex={0}
              >
                {markedLines(lines, blockedNames).map((line, index) => (
                  <div
                    // biome-ignore lint/suspicious/noArrayIndexKey: the lines of a text written from the form
                    key={index}
                    className="sk-line"
                    data-blocked={line.blocked || undefined}
                  >
                    <span className="sk-ln" aria-hidden="true">
                      {index + 1}
                    </span>
                    <span className="sk-text">{line.text || ' '}</span>
                  </div>
                ))}
              </section>
              <p className="sb-side-foot">
                {template
                  ? 'With a template, containers only pin versions of the template’s workers.'
                  : 'Every container here starts in each group, with what it depends on.'}
              </p>
            </aside>
          )}
        </div>

        <footer className="sk-sheet-foot">
          <span
            className="sk-status sb-status"
            role={createError ? 'alert' : 'status'}
            data-tone={alert ? 'alert' : 'faint'}
          >
            {status.blocked || createError ? (
              <CircleX size={16} aria-hidden="true" />
            ) : null}
            <span id="sb-status">{statusText}</span>
          </span>
          <button
            type="button"
            className="sk-btn"
            aria-disabled={creating || undefined}
            onClick={() => {
              if (!creating) onClose()
            }}
          >
            Cancel
          </button>
          <button
            type="button"
            className="sk-btn sk-btn-primary"
            aria-disabled={status.blocked || creating || undefined}
            aria-busy={creating || undefined}
            aria-describedby="sb-status"
            onClick={() => void create()}
          >
            {creating ? 'Creating…' : 'Create stack'}
          </button>
        </footer>
      </DialogContent>
    </Dialog>
  )
}

/** A declared worker: its name and reference, its version menu, Remove, and
 *  what it does. */
function DeclaredRow({
  entry,
  verdict,
  open,
  onToggle,
  onChange,
  onRemove,
  onRetry,
}: {
  entry: Declared
  verdict: Verdict | null
  open: boolean
  onToggle: (button: HTMLButtonElement) => void
  onChange: (change: Partial<Declared>) => void
  onRemove: () => void
  onRetry: (worker: string) => void
}) {
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
            onClick={(event) => onToggle(event.currentTarget)}
          >
            <span className="sk-mono">{pinLabel(entry)}</span>
            <ChevronDown size={14} aria-hidden="true" />
          </button>
          {open ? <VersionMenu entry={entry} onChange={onChange} /> : null}
        </span>
        <button
          type="button"
          className="sk-btn sk-btn-small sk-btn-icon"
          aria-label={`Remove ${entry.name}`}
          title="Remove"
          onClick={onRemove}
        >
          <X size={16} aria-hidden="true" />
        </button>
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
    </li>
  )
}
