import { useEffect, useState } from 'react'

const PHONE = '(max-width: 639px)'

/** True on a phone-sized viewport (under 640 px), where the header drops its
 *  title, sections open as a sheet and the primary action pins to the bottom. */
export function useViewportPhone() {
  const [phone, setPhone] = useState(
    () => typeof window !== 'undefined' && window.matchMedia?.(PHONE).matches,
  )
  useEffect(() => {
    const query = window.matchMedia?.(PHONE)
    if (!query) return
    const update = () => setPhone(query.matches)
    update()
    query.addEventListener('change', update)
    return () => query.removeEventListener('change', update)
  }, [])
  return Boolean(phone)
}
