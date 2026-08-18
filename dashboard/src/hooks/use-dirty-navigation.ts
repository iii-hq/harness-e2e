import { useEffect, useRef, useState } from 'react'

const warning = 'You have unsaved changes. Leave this page and discard them?'

/** Protect a dirty editor from hash navigation, pane close and tab refresh. */
export function useDirtyNavigation(dirty: boolean) {
  const acceptedHash = useRef(
    typeof window === 'undefined' ? '' : window.location.hash,
  )
  const pendingHash = useRef<string | null>(null)
  const [, rerender] = useState(0)

  useEffect(() => {
    const handleHashChange = () => {
      const nextHash = window.location.hash
      if (!dirty || nextHash === acceptedHash.current) {
        acceptedHash.current = nextHash
        return
      }
      pendingHash.current = nextHash
      window.history.replaceState(
        window.history.state,
        '',
        `${window.location.pathname}${window.location.search}${acceptedHash.current}`,
      )
      rerender((value) => value + 1)
    }
    const handleBeforeUnload = (event: BeforeUnloadEvent) => {
      if (!dirty) return
      event.preventDefault()
      event.returnValue = warning
    }
    window.addEventListener('hashchange', handleHashChange)
    window.addEventListener('beforeunload', handleBeforeUnload)
    return () => {
      window.removeEventListener('hashchange', handleHashChange)
      window.removeEventListener('beforeunload', handleBeforeUnload)
    }
  }, [dirty])

  return {
    pendingHash: pendingHash.current,
    confirmNavigation() {
      const nextHash = pendingHash.current
      if (!nextHash) return
      pendingHash.current = null
      acceptedHash.current = nextHash
      window.location.hash = nextHash
      rerender((value) => value + 1)
    },
    cancelNavigation() {
      pendingHash.current = null
      rerender((value) => value + 1)
    },
    warning,
  }
}
