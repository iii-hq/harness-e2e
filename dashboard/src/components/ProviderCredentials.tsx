import {
  ConfirmDialog,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from '@iii-dev/console-ui'
import { Download, Plus, Trash2 } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'
import type {
  Credential,
  CredentialsImport,
  DashboardDataBridge,
} from '@/lib/dashboard-data-source'

// Drawn by pages/stacks-page.css, the one page that shows it.

const NAME = /^[A-Z][A-Z0-9_]*$/

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

/** What an import from the worker's environment did, in one sentence. */
export function importSummary({ found, not_found }: CredentialsImport) {
  if (found.length === 0)
    return `None of the known keys is in the worker's environment (${not_found.join(', ')}).`
  return `Set from the worker's environment: ${found.join(', ')}.${
    not_found.length ? ` Not found there: ${not_found.join(', ')}.` : ''
  }`
}

export function credentialStatus(credential: Credential) {
  if (credential.source === 'console') return 'Set here'
  if (credential.source === 'provider_env_file')
    return 'Set by the worker’s provider_env_file'
  return 'Not set'
}

/** Sets one credential: its name (fixed when it is a row's) and its value,
 *  which is sent once and never shown again. */
function CredentialDialog({
  bridge,
  name: fixed,
  narrow,
  onClose,
  onSaved,
}: {
  bridge: DashboardDataBridge
  name: string | null
  narrow: boolean
  onClose: () => void
  onSaved: (credentials: Credential[], name: string) => void
}) {
  const [name, setName] = useState(fixed ?? '')
  const [value, setValue] = useState('')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const nameError = name !== '' && !NAME.test(name)

  const save = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!NAME.test(name) || value.trim() === '') return
    setSaving(true)
    setError(null)
    try {
      const { credentials } = await bridge.setCredential(name, value)
      setValue('')
      onSaved(credentials, name)
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setSaving(false)
    }
  }
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !saving) onClose()
      }}
    >
      <DialogContent
        className="sk-new sk-cred-dialog"
        data-narrow={narrow || undefined}
        onOpenAutoFocus={(event) => {
          // A row's name is fixed: the value is what is left to type.
          if (!fixed) return
          event.preventDefault()
          document.getElementById('credential-value')?.focus()
        }}
      >
        <header className="sk-sheet-head">
          <DialogTitle className="sk-new-title">
            {fixed ? `Set ${fixed}` : 'Add a credential'}
          </DialogTitle>
          <DialogDescription className="sk-sheet-desc">
            Docker executions receive it as an environment variable. The value
            stays on this machine, readable only by the worker, and is never
            shown again.
          </DialogDescription>
        </header>
        <form
          id="credential-form"
          className="sk-new-body"
          onSubmit={save}
          noValidate
        >
          <div className="sk-field">
            <label className="sk-field-label" htmlFor="credential-name">
              Name
            </label>
            <input
              id="credential-name"
              className="sk-input sk-input-mono"
              value={name}
              placeholder="OPENAI_API_KEY"
              autoComplete="off"
              spellCheck={false}
              readOnly={Boolean(fixed)}
              disabled={saving}
              aria-invalid={nameError || undefined}
              aria-describedby="credential-name-hint"
              onChange={(event) => setName(event.target.value.trim())}
            />
            <span
              id="credential-name-hint"
              className="sk-hint"
              data-tone={nameError ? 'warn' : undefined}
            >
              {nameError
                ? 'Capital letters, digits and _, starting with a letter.'
                : 'The environment variable a provider reads, as OPENAI_API_KEY.'}
            </span>
          </div>
          <div className="sk-field">
            <label className="sk-field-label" htmlFor="credential-value">
              Value
            </label>
            <input
              id="credential-value"
              className="sk-input sk-input-mono"
              type="password"
              value={value}
              placeholder="Paste the key"
              autoComplete="off"
              spellCheck={false}
              disabled={saving}
              aria-invalid={error ? true : undefined}
              aria-describedby={
                error
                  ? 'credential-value-hint credential-error'
                  : 'credential-value-hint'
              }
              onChange={(event) => setValue(event.target.value)}
            />
            <span id="credential-value-hint" className="sk-hint">
              Sent once, as a field the iii SDKs leave out of the traces they
              record.
            </span>
            {error ? (
              <span
                id="credential-error"
                className="sk-hint"
                role="alert"
                data-tone="alert"
              >
                {error}
              </span>
            ) : null}
          </div>
        </form>
        <footer className="sk-new-foot sk-cred-foot">
          <button
            type="button"
            className="sk-btn"
            disabled={saving}
            onClick={onClose}
          >
            Cancel
          </button>
          <button
            type="submit"
            form="credential-form"
            className="sk-btn sk-btn-primary"
            disabled={saving || !NAME.test(name) || value.trim() === ''}
            aria-busy={saving || undefined}
          >
            {saving ? 'Saving…' : 'Save credential'}
          </button>
        </footer>
      </DialogContent>
    </Dialog>
  )
}

/** The provider credentials Docker executions receive: each by name, set or
 *  not; set, replaced or deleted here, or imported from the worker's own
 *  environment. No value ever comes back. */
export function ProviderCredentials({
  bridge,
  narrow = false,
}: {
  bridge: DashboardDataBridge | null
  narrow?: boolean
}) {
  const [credentials, setCredentials] = useState<Credential[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState('')
  const [busy, setBusy] = useState(false)
  const [editing, setEditing] = useState<{ name: string | null } | null>(null)
  const [deleting, setDeleting] = useState<Credential | null>(null)

  const load = useCallback(async () => {
    if (!bridge) return
    try {
      setCredentials((await bridge.listCredentials()).credentials)
      setError(null)
    } catch (cause) {
      setError(errorText(cause))
    }
  }, [bridge])
  useEffect(() => {
    void load()
  }, [load])

  const act = async (action: () => Promise<void>) => {
    setBusy(true)
    setError(null)
    try {
      await action()
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setBusy(false)
    }
  }
  const importFromMachine = () =>
    act(async () => {
      if (!bridge) return
      const result = await bridge.importCredentials()
      setCredentials(result.credentials)
      setNotice(importSummary(result))
    })
  // A delete in flight: a second confirmation of it is ignored.
  const removing = useRef(false)
  const remove = async (credential: Credential) => {
    if (!bridge || removing.current) return
    removing.current = true
    setDeleting(null)
    try {
      await act(async () => {
        setCredentials(
          (await bridge.deleteCredential(credential.name)).credentials,
        )
        setNotice(`${credential.name} deleted.`)
      })
    } finally {
      removing.current = false
    }
  }

  return (
    <section
      className="sk-block"
      aria-labelledby="provider-credentials-heading"
      data-credentials
    >
      <div className="sk-block-head sk-cred-head">
        <h2 className="ds-label" id="provider-credentials-heading">
          Provider credentials
        </h2>
        <span className="sk-block-note sk-cred-note">
          What a Docker execution's providers read, as environment variables.
          Kept on this machine, readable only by the worker; only names are
          shown.
        </span>
        <button
          className="sk-btn sk-btn-small"
          type="button"
          disabled={!bridge || busy}
          onClick={() => void importFromMachine()}
        >
          <Download size={14} aria-hidden="true" />
          Import from this machine
        </button>
        <button
          className="sk-btn sk-btn-small sk-btn-fill"
          type="button"
          disabled={!bridge || busy}
          onClick={() => setEditing({ name: null })}
        >
          <Plus size={14} aria-hidden="true" />
          Add credential
        </button>
      </div>
      {notice ? (
        <p className="sk-cred-notice" role="status">
          {notice}
        </p>
      ) : null}
      {error ? (
        <p className="sk-cred-notice" role="alert" data-tone="alert">
          {error}
        </p>
      ) : null}
      {credentials === null ? null : (
        <table
          className="sk-creds"
          aria-labelledby="provider-credentials-heading"
          data-narrow={narrow || undefined}
        >
          <thead>
            <tr className="sk-cred-row sk-cred-columns">
              <th scope="col">Name</th>
              <th scope="col">Status</th>
              <th scope="col">Read by</th>
              <th scope="col">
                <span className="ds-visually-hidden">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {credentials.map((credential) => (
              <tr
                key={credential.name}
                className="sk-cred-row"
                data-credential={credential.name}
              >
                <td className="sk-cred-name">{credential.name}</td>
                <td
                  className="sk-cred-status"
                  data-status
                  data-set={credential.set || undefined}
                >
                  <span
                    className="sk-dot"
                    data-tone={credential.set ? 'ok' : undefined}
                    aria-hidden="true"
                  />
                  {credentialStatus(credential)}
                </td>
                <td className="sk-cred-by">
                  {credential.providers.length
                    ? credential.providers.join(', ')
                    : '—'}
                </td>
                <td className="sk-cred-actions">
                  <button
                    className="sk-btn sk-btn-small"
                    type="button"
                    aria-label={`${credential.source === 'console' ? 'Replace' : 'Set'} ${credential.name}`}
                    disabled={!bridge || busy}
                    onClick={() => setEditing({ name: credential.name })}
                  >
                    {credential.source === 'console' ? 'Replace' : 'Set'}
                  </button>
                  {credential.source === 'console' ? (
                    <button
                      className="sk-btn sk-btn-small sk-btn-icon"
                      type="button"
                      aria-label={`Delete ${credential.name}`}
                      title="Delete credential"
                      disabled={!bridge || busy}
                      onClick={() => setDeleting(credential)}
                    >
                      <Trash2 size={14} aria-hidden="true" />
                    </button>
                  ) : null}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <p className="sk-cred-foot-note">
        Subscription providers (openai-codex, claude-code) need nothing here:
        each group gets an access token from this machine's <code>codex</code>{' '}
        or <code>claude</code> login, refreshed when it would not last the
        group.
      </p>
      {editing && bridge ? (
        <CredentialDialog
          bridge={bridge}
          name={editing.name}
          narrow={narrow}
          onClose={() => setEditing(null)}
          onSaved={(next, name) => {
            setCredentials(next)
            setNotice(`${name} saved; the next Docker phase receives it.`)
            setEditing(null)
          }}
        />
      ) : null}
      <ConfirmDialog
        open={deleting !== null}
        onOpenChange={(open) => {
          if (!open) setDeleting(null)
        }}
        title={`Delete ${deleting?.name ?? 'credential'}?`}
        description="Docker executions started afterwards no longer receive it."
        confirmLabel="Delete credential"
        tone="danger"
        onConfirm={() => deleting && void remove(deleting)}
      />
    </section>
  )
}
