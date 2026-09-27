import {
  ChevronLeft,
  ChevronRight,
  Download,
  Minus,
  Plus,
  X,
} from 'lucide-react'
import {
  type KeyboardEvent,
  type PointerEvent,
  useEffect,
  useRef,
  useState,
} from 'react'
import {
  type DashboardDataBridge,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import { type ScreenshotEntry, screenshotSource } from '@/lib/screenshots'
import './execution-page.css'

export type Image = { source: string } | { error: string } | undefined

/** The screenshots' bytes, read once per set of keys. */
export function useScreenshotImages(
  bridge: DashboardDataBridge | null,
  screenshots: ScreenshotEntry[],
) {
  const [images, setImages] = useState<Record<string, Image>>({})
  // biome-ignore lint/correctness/useExhaustiveDependencies: keyed by the screenshots' keys
  useEffect(() => {
    if (screenshots.length === 0) return
    let cancelled = false
    void (bridge ? Promise.resolve(bridge) : getDashboardDataBridge()).then(
      (reader) => {
        for (const screenshot of screenshots)
          screenshotSource(reader.readEvidence, screenshot)
            .then((source) => ({ source }))
            .catch((cause: unknown) => ({
              error: cause instanceof Error ? cause.message : String(cause),
            }))
            .then((image) => {
              if (!cancelled)
                setImages((current) => ({
                  ...current,
                  [screenshot.key]: image,
                }))
            })
      },
    )
    return () => {
      cancelled = true
    }
  }, [bridge, screenshots.map((screenshot) => screenshot.key).join('|')])
  return images
}

/** A run's screenshots as thumbnails; one opens the full-size viewer. */
export function ScreenshotGallery({
  bridge,
  screenshots,
  heading = true,
}: {
  bridge: DashboardDataBridge | null
  screenshots: ScreenshotEntry[]
  /** False where the page already titles the section. */
  heading?: boolean
}) {
  const images = useScreenshotImages(bridge, screenshots)
  const [open, setOpen] = useState<number | null>(null)
  const triggers = useRef<Array<HTMLButtonElement | null>>([])
  if (screenshots.length === 0) return null
  return (
    <section className="ep-shots" aria-label="Screenshots">
      {heading ? (
        <p className="ep-faint">
          Screenshots · {screenshots.length} · select one to open it full size
        </p>
      ) : null}
      <ul className="ep-shot-grid">
        {screenshots.map((screenshot, index) => {
          const image = images[screenshot.key]
          return (
            <li key={screenshot.key}>
              <button
                ref={(element) => {
                  triggers.current[index] = element
                }}
                type="button"
                className="ep-shot"
                data-screenshot={screenshot.key}
                aria-label={`Open screenshot: ${screenshot.caption}`}
                onClick={() => setOpen(index)}
              >
                {image && 'source' in image ? (
                  <img src={image.source} alt="" loading="lazy" />
                ) : (
                  <span className="ep-faint" role="status">
                    {image ? image.error : 'loading…'}
                  </span>
                )}
                <span className="ep-shot-caption">{screenshot.caption}</span>
              </button>
            </li>
          )
        })}
      </ul>
      {open !== null ? (
        <ScreenshotViewer
          screenshots={screenshots}
          images={images}
          index={open}
          onIndex={setOpen}
          onClose={() => {
            const back = triggers.current[open]
            setOpen(null)
            back?.focus()
          }}
        />
      ) : null}
    </section>
  )
}

const ZOOMS = [1, 1.5, 2, 3, 4]

/** Full size: fit, zoom (1–4×), drag to pan when zoomed, ←/→ for the
 *  previous/next screenshot, Esc closes and focus returns to the thumbnail. */
export function ScreenshotViewer({
  screenshots,
  images,
  index,
  onIndex,
  onClose,
  evidenceHref,
}: {
  screenshots: ScreenshotEntry[]
  images: Record<string, Image>
  index: number
  onIndex: (index: number) => void
  onClose: () => void
  /** The evidence record the screenshots belong to, when not already on it. */
  evidenceHref?: string
}) {
  const [zoom, setZoom] = useState(0)
  const [dims, setDims] = useState<{
    w: number
    h: number
    fit: number
  } | null>(null)
  const [offset, setOffset] = useState({ x: 0, y: 0 })
  const drag = useRef<{ x: number; y: number } | null>(null)
  const root = useRef<HTMLDivElement>(null)
  const screenshot = screenshots[index]
  const image = screenshot ? images[screenshot.key] : undefined
  const scale = ZOOMS[zoom] ?? 1

  useEffect(() => {
    root.current?.focus()
  }, [])
  // biome-ignore lint/correctness/useExhaustiveDependencies: reset on a new screenshot
  useEffect(() => {
    setZoom(0)
    setOffset({ x: 0, y: 0 })
    setDims(null)
  }, [index])

  const go = (by: number) =>
    onIndex((index + by + screenshots.length) % screenshots.length)
  const setScale = (next: number) => {
    const clamped = Math.max(0, Math.min(ZOOMS.length - 1, next))
    setZoom(clamped)
    if (clamped === 0) setOffset({ x: 0, y: 0 })
  }
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      onClose()
    } else if (event.key === 'ArrowRight' && screenshots.length > 1) go(1)
    else if (event.key === 'ArrowLeft' && screenshots.length > 1) go(-1)
    else if (event.key === '+' || event.key === '=') setScale(zoom + 1)
    else if (event.key === '-') setScale(zoom - 1)
    else if (event.key === '0') setScale(0)
  }
  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (zoom === 0) return
    drag.current = { x: event.clientX - offset.x, y: event.clientY - offset.y }
    event.currentTarget.setPointerCapture(event.pointerId)
  }
  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    if (!drag.current) return
    setOffset({
      x: event.clientX - drag.current.x,
      y: event.clientY - drag.current.y,
    })
  }

  if (!screenshot) return null
  const titleId = `viewer-${index}`
  const source = image && 'source' in image ? image.source : null
  return (
    <div
      ref={root}
      className="ep-viewer"
      role="dialog"
      aria-modal="true"
      aria-labelledby={titleId}
      tabIndex={-1}
      onKeyDown={onKeyDown}
    >
      <header className="ep-viewer-bar">
        <div className="ep-viewer-heading">
          <h2 id={titleId} className="ep-viewer-title">
            {screenshot.caption}
          </h2>
          <span className="ep-faint ep-mono">
            {[
              `run ${screenshot.runId.slice(0, 8)}`,
              dims ? `${dims.w} × ${dims.h}` : null,
            ]
              .filter(Boolean)
              .join(' · ')}
          </span>
        </div>
        <span className="ep-faint ep-mono">
          {index + 1} of {screenshots.length}
        </span>
        <fieldset className="ep-viewer-zoom">
          <legend className="ep-sr">Zoom</legend>
          <button
            type="button"
            className="ep-viewer-button"
            aria-label="Zoom out"
            disabled={zoom === 0}
            onClick={() => setScale(zoom - 1)}
          >
            <Minus size={16} aria-hidden="true" />
          </button>
          <output className="ep-viewer-pct ep-mono" aria-live="polite">
            {`${Math.round(scale * (dims?.fit ?? 1) * 100)}%`}
          </output>
          <button
            type="button"
            className="ep-viewer-button"
            aria-label="Zoom in"
            disabled={zoom === ZOOMS.length - 1}
            onClick={() => setScale(zoom + 1)}
          >
            <Plus size={16} aria-hidden="true" />
          </button>
          <button
            type="button"
            className="ep-viewer-button ep-viewer-fit"
            aria-pressed={zoom === 0}
            onClick={() => setScale(0)}
          >
            Fit
          </button>
        </fieldset>
        {evidenceHref ? (
          <a className="ep-viewer-button ep-viewer-link" href={evidenceHref}>
            Evidence record
          </a>
        ) : null}
        {source ? (
          <a
            className="ep-viewer-button"
            href={source}
            download={
              screenshot.caption.replace(/[^a-z0-9._-]+/gi, '-') ||
              'screenshot.png'
            }
            aria-label={`Download ${screenshot.caption}`}
            title="Download"
          >
            <Download size={16} aria-hidden="true" />
          </a>
        ) : null}
        <button
          type="button"
          className="ep-viewer-button"
          aria-label="Close"
          title="Close"
          onClick={onClose}
        >
          <X size={16} aria-hidden="true" />
        </button>
      </header>
      <div
        className="ep-viewer-stage"
        data-zoomed={zoom > 0 || undefined}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={() => {
          drag.current = null
        }}
      >
        {source ? (
          <img
            src={source}
            alt={screenshot.caption}
            draggable={false}
            onLoad={(event) =>
              setDims({
                w: event.currentTarget.naturalWidth,
                h: event.currentTarget.naturalHeight,
                fit:
                  event.currentTarget.naturalWidth > 0
                    ? event.currentTarget.clientWidth /
                      event.currentTarget.naturalWidth
                    : 1,
              })
            }
            style={{
              transform: `translate(${offset.x}px, ${offset.y}px) scale(${scale})`,
            }}
          />
        ) : (
          <span className="ep-faint" role="status">
            {image && 'error' in image ? image.error : 'loading…'}
          </span>
        )}
        {screenshots.length > 1 ? (
          <>
            <button
              type="button"
              className="ep-viewer-button ep-viewer-prev"
              aria-label="Previous screenshot"
              onPointerDown={(event) => event.stopPropagation()}
              onClick={() => go(-1)}
            >
              <ChevronLeft size={16} aria-hidden="true" />
            </button>
            <button
              type="button"
              className="ep-viewer-button ep-viewer-next"
              aria-label="Next screenshot"
              onPointerDown={(event) => event.stopPropagation()}
              onClick={() => go(1)}
            >
              <ChevronRight size={16} aria-hidden="true" />
            </button>
          </>
        ) : null}
      </div>
      <footer className="ep-viewer-bar ep-viewer-foot">
        {screenshots.length > 1 ? (
          <nav
            className="ep-viewer-thumbs"
            aria-label="Screenshots of this run"
          >
            {screenshots.map((entry, position) => {
              const thumb = images[entry.key]
              return (
                <button
                  key={entry.key}
                  type="button"
                  className="ep-viewer-thumb"
                  aria-current={position === index || undefined}
                  data-selected={position === index}
                  title={entry.caption}
                  onClick={() => onIndex(position)}
                >
                  <span className="ep-viewer-thumb-img">
                    {thumb && 'source' in thumb ? (
                      <img src={thumb.source} alt="" />
                    ) : null}
                  </span>
                  <span className="ep-viewer-thumb-label">{entry.caption}</span>
                </button>
              )
            })}
          </nav>
        ) : null}
        <p className="ep-viewer-keys ep-faint">
          {screenshots.length > 1 ? (
            <span>
              <kbd>←</kbd>
              <kbd>→</kbd> screenshot
            </span>
          ) : null}
          <span>
            <kbd>+</kbd>
            <kbd>−</kbd> zoom
          </span>
          <span>
            <kbd>0</kbd> fit
          </span>
          <span>drag to pan when zoomed</span>
          <span>
            <kbd>Esc</kbd> close
          </span>
        </p>
      </footer>
    </div>
  )
}
