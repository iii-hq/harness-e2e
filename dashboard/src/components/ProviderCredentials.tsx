import { useCallback, useEffect, useState } from 'react'
import {
  buttonClassName,
  DataTable,
  Dialog,
  Field,
  fieldDescribedBy,
  Input,
  PageHeader,
} from '@/design-system'
import type {
  Credential,
  CredentialsImport,
  DashboardDataBridge,
} from '@/lib/dashboard-data-source'

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
  if (credential.source === 'console') return 'set here'
  if (credential.source === 'provider_env_file')
    return 'set by the worker’s provider_env_file'
  return 'not set'
}

/** Sets one credential: its name (fixed when it is a row's) and its value,
 *  which is sent once and never shown again. */
function CredentialDialog({
  bridge,
  name: fixed,
  onClose,
  onSaved,
}: {
  bridge: DashboardDataBridge
  name: string | null
  onClose: () => void
  onSaved: (credentials: Credential[], name: string) => void
}) {
  const [name, setName] = useState(fixed ?? '')
  const [value, setValue] = useState('')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const nameError =
    name !== '' && !NAME.test(name)
      ? 'Capital letters, digits and _, starting with a letter.'
      : undefined

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
      onClose={() => !saving && onClose()}
      size="sm"
      kicker="Provider credential"
      title={fixed ? 'Set credential' : 'Add a credential'}
      description="Docker executions receive it as an environment variable. The value stays on this machine, readable only by the worker, and is never shown again."
      closeLabel="Close credential"
      className="ds-root"
      footer={
        <div className="flex justify-end gap-2">
          <button
            className={buttonClassName({ variant: 'secondary' })}
            type="button"
            onClick={onClose}
            disabled={saving}
          >
            cancel
          </button>
          <button
            className={buttonClassName({ variant: 'primary' })}
            type="submit"
            form="credential-form"
            disabled={saving || !NAME.test(name) || value.trim() === ''}
            aria-busy={saving}
          >
            {saving ? 'saving…' : 'save credential'}
          </button>
        </div>
      }
    >
      <form
        id="credential-form"
        className="grid min-w-0 gap-4"
        onSubmit={save}
        noValidate
      >
        <Field
          label="Name"
          htmlFor="credential-name"
          hint={
            fixed
              ? undefined
              : 'The environment variable a provider reads, as OPENAI_API_KEY.'
          }
          error={nameError}
        >
          <Input
            id="credential-name"
            value={name}
            autoComplete="off"
            spellCheck={false}
            aria-invalid={nameError ? true : undefined}
            aria-describedby={fieldDescribedBy('credential-name', {
              hint: !fixed,
              error: Boolean(nameError),
            })}
            onChange={(event) => setName(event.target.value.trim())}
            readOnly={Boolean(fixed)}
            className={fixed ? 'font-mono' : undefined}
            disabled={saving}
          />
        </Field>
        <Field label="Value" htmlFor="credential-value" error={error}>
          <Input
            id="credential-value"
            type="password"
            value={value}
            autoComplete="off"
            spellCheck={false}
            aria-invalid={error ? true : undefined}
            aria-describedby={fieldDescribedBy('credential-value', {
              error: Boolean(error),
            })}
            onChange={(event) => setValue(event.target.value)}
            disabled={saving}
          />
        </Field>
      </form>
    </Dialog>
  )
}

/** The provider credentials Docker executions receive: each by name, set or
 *  not; set, replaced or deleted here, or imported from the worker's own
 *  environment. No value ever comes back. */
export function ProviderCredentials({
  bridge,
}: {
  bridge: DashboardDataBridge | null
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
  const remove = (credential: Credential) =>
    act(async () => {
      if (!bridge) return
      setCredentials(
        (await bridge.deleteCredential(credential.name)).credentials,
      )
      setDeleting(null)
      setNotice(`${credential.name} deleted.`)
    })

  return (
    <section
      className="mt-10"
      aria-labelledby="provider-credentials-heading"
      data-credentials
    >
      <PageHeader
        headingLevel={2}
        headingId="provider-credentials-heading"
        title="provider credentials"
        summary="What the providers of a Docker execution read, as environment variables: kept on this machine, readable only by the worker, never in an execution's folder or evidence. Only names are shown."
        actions={
          <span className="inline-flex flex-wrap gap-2">
            <button
              className={buttonClassName({ variant: 'secondary' })}
              type="button"
              disabled={!bridge || busy}
              onClick={() => void importFromMachine()}
            >
              import from this machine
            </button>
            <button
              className={buttonClassName({ variant: 'primary' })}
              type="button"
              disabled={!bridge || busy}
              onClick={() => setEditing({ name: null })}
            >
              add credential
            </button>
          </span>
        }
      />
      <p className="mt-3 text-sm text-ink-soft" role="status">
        {notice}
      </p>
      {error ? (
        <p className="mt-2 text-sm text-danger" role="alert">
          {error}
        </p>
      ) : null}
      {credentials === null ? null : (
        <div className="mt-4">
          <DataTable caption="Provider credentials" collapse minWidth="40rem">
            <thead>
              <tr>
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
                <tr key={credential.name} data-credential={credential.name}>
                  <td data-label="Name" className="font-mono text-xs text-ink">
                    {credential.name}
                  </td>
                  <td
                    data-label="Status"
                    className={
                      credential.set
                        ? 'text-xs text-ink'
                        : 'text-xs text-ink-muted'
                    }
                  >
                    {credentialStatus(credential)}
                  </td>
                  <td data-label="Read by" className="text-xs text-ink-soft">
                    {credential.providers.length
                      ? credential.providers.join(', ')
                      : '—'}
                  </td>
                  <td className="text-right">
                    <span className="inline-flex flex-wrap justify-end gap-2">
                      <button
                        className={buttonClassName({
                          variant: 'secondary',
                          size: 'compact',
                        })}
                        type="button"
                        aria-label={`${credential.source === 'console' ? 'Replace' : 'Set'} ${credential.name}`}
                        disabled={!bridge || busy}
                        onClick={() => setEditing({ name: credential.name })}
                      >
                        {credential.source === 'console' ? 'replace' : 'set'}
                      </button>
                      {credential.source === 'console' ? (
                        <button
                          className={buttonClassName({
                            variant: 'quiet',
                            size: 'compact',
                          })}
                          type="button"
                          aria-label={`Delete ${credential.name}`}
                          disabled={!bridge || busy}
                          onClick={() => setDeleting(credential)}
                        >
                          delete
                        </button>
                      ) : null}
                    </span>
                  </td>
                </tr>
              ))}
            </tbody>
          </DataTable>
        </div>
      )}
      {editing && bridge ? (
        <CredentialDialog
          bridge={bridge}
          name={editing.name}
          onClose={() => setEditing(null)}
          onSaved={(next, name) => {
            setCredentials(next)
            setNotice(`${name} saved; the next Docker phase receives it.`)
            setEditing(null)
          }}
        />
      ) : null}
      <Dialog
        open={deleting !== null}
        onClose={() => !busy && setDeleting(null)}
        size="sm"
        title={`Delete ${deleting?.name ?? 'credential'}?`}
        description="Docker executions started afterwards no longer receive it."
        footer={
          <div className="flex justify-end gap-2">
            <button
              type="button"
              className={buttonClassName({ variant: 'secondary' })}
              disabled={busy}
              onClick={() => setDeleting(null)}
            >
              cancel
            </button>
            <button
              type="button"
              className={buttonClassName({ variant: 'primary' })}
              disabled={busy}
              aria-busy={busy}
              onClick={() => deleting && void remove(deleting)}
            >
              {busy ? 'deleting…' : 'delete credential'}
            </button>
          </div>
        }
      />
    </section>
  )
}
