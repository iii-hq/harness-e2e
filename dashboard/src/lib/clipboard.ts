/** Copies text; false when the browser would not.
 *
 *  `navigator.clipboard` exists only on https pages and localhost, so a
 *  Console served on http://host (the dev Console at iii.local) falls back to
 *  the older copy command, which still works from a click. */
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text)
    return true
  } catch {
    return commandCopy(text)
  }
}

function commandCopy(text: string) {
  if (typeof document === 'undefined') return false
  // The field sits beside what has focus, so a dialog's focus trap keeps it.
  let active = document.activeElement
  while (active?.shadowRoot?.activeElement)
    active = active.shadowRoot.activeElement
  const area = document.createElement('textarea')
  area.value = text
  area.setAttribute('readonly', '')
  area.setAttribute('aria-hidden', 'true')
  area.style.position = 'fixed'
  area.style.top = '0'
  area.style.opacity = '0'
  ;(active?.parentElement ?? document.body).append(area)
  area.select()
  try {
    return document.execCommand('copy')
  } catch {
    return false
  } finally {
    area.remove()
    ;(active as HTMLElement | null)?.focus?.()
  }
}
