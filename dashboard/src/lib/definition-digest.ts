/**
 * A scenario definition is identified by its content digest, `behavior_sha256`
 * (`sha256:<64 hex>`), sealed when the case is materialized. Two runs
 * evaluated by the same definition share the digest; any change to the prompt,
 * the execution policy, the criteria or the contract changes it.
 */

/** The server's key for a slot whose case never materialized. */
export const UNMATERIALIZED_DEFINITION = 'unmaterialized'

/** What a reader sees for a slot the server grouped as unmaterialized. */
export const UNMATERIALIZED_LABEL = 'not materialized'

/**
 * The readable form of a definition digest: its first eight hex characters.
 * The full digest belongs in the `title` next to it (see `definitionTitle`).
 */
export function shortDefinition(
  digest: string | null | undefined,
): string | null {
  if (!digest) return null
  if (digest === UNMATERIALIZED_DEFINITION) return UNMATERIALIZED_LABEL
  return digest.replace(/^sha256:/, '').slice(0, 8)
}

/** The tooltip that carries the whole digest behind its short form. */
export function definitionTitle(
  digest: string | null | undefined,
): string | undefined {
  if (!digest) return undefined
  return digest === UNMATERIALIZED_DEFINITION
    ? 'This slot never materialized a case, so it carries no definition digest.'
    : digest
}
