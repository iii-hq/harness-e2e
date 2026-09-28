import { useEffect, useRef, useState } from 'react'

/** The rendered width of an element, so an SVG's units are its pixels. */
export function useMeasuredWidth(fallback: number) {
  const ref = useRef<HTMLDivElement>(null)
  const [width, setWidth] = useState(fallback)
  useEffect(() => {
    const element = ref.current
    if (!element || typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver((entries) => {
      const next = entries[0]?.contentRect.width
      if (next) setWidth(Math.round(next))
    })
    observer.observe(element)
    return () => observer.disconnect()
  }, [])
  return { ref, width }
}
