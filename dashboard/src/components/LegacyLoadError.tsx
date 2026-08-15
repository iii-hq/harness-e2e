import { AlertTriangle } from 'lucide-react'

export function LegacyLoadError({ error }: { error: Error | null }) {
  if (!error) return null

  return (
    <div className="app-load-error" role="alert">
      <AlertTriangle size={18} aria-hidden="true" />
      <div>
        <strong>Dashboard data could not be loaded</strong>
        <span>{error.message}</span>
      </div>
      <button type="button" onClick={() => window.location.reload()}>
        Retry
      </button>
    </div>
  )
}
