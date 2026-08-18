import { useCallback, useEffect, useRef, useState } from 'react'

/**
 * Injected pages live in tabs and split panes, so viewport media queries are
 * not enough to choose an interaction model. This hook observes the element
 * that actually owns the workspace and ignores hidden zero-width panes.
 */
export function useContainerNarrow(
  threshold: number,
): [(node: HTMLElement | null) => void, boolean] {
  const [narrow, setNarrow] = useState(false)
  const nodeRef = useRef<HTMLElement | null>(null)
  const observerRef = useRef<ResizeObserver | null>(null)

  const measure = useCallback(
    (node: HTMLElement | null) => {
      if (!node) {
        setNarrow(false)
        return
      }
      const width = node.getBoundingClientRect().width
      if (width > 0) setNarrow(width < threshold)
    },
    [threshold],
  )

  const ref = useCallback(
    (node: HTMLElement | null) => {
      observerRef.current?.disconnect()
      observerRef.current = null
      nodeRef.current = node
      measure(node)
      if (!node || typeof ResizeObserver === 'undefined') return
      const observer = new ResizeObserver(([entry]) => {
        const width = entry?.contentRect.width ?? 0
        if (width > 0) setNarrow(width < threshold)
      })
      observer.observe(node)
      observerRef.current = observer
    },
    [measure, threshold],
  )

  useEffect(() => () => observerRef.current?.disconnect(), [])

  return [ref, narrow]
}
