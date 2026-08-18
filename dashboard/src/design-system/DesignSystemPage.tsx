import { useGSAP } from '@gsap/react'
import gsap from 'gsap'
import { ScrollTrigger } from 'gsap/ScrollTrigger'
import { ChevronLeft, ChevronRight, Moon, Sun } from 'lucide-react'
import { type CSSProperties, useRef, useState } from 'react'
import {
  Button,
  buttonClassName,
  MetricCard,
  PageHeader,
  Panel,
  StatusBadge,
} from './primitives'

gsap.registerPlugin(useGSAP, ScrollTrigger)

const architectureItems = [
  {
    id: 'signal',
    title: 'Current signal',
    copy: 'Lead with the objective system outcome and the first actionable failure.',
    image: 'https://picsum.photos/seed/harness-signal/1200/900',
  },
  {
    id: 'evidence',
    title: 'Evidence boundary',
    copy: 'Keep deterministic gates authoritative while AI guidance remains advisory.',
    image: 'https://picsum.photos/seed/harness-evidence/1200/900',
  },
  {
    id: 'decision',
    title: 'Decision path',
    copy: 'Show baseline deltas, direction, provenance, and the next operational action.',
    image: 'https://picsum.photos/seed/harness-decision/1200/900',
  },
]

const guidance = [
  {
    title: 'Protect the objective outcome',
    copy: 'A persuasive recommendation never overrides a hard gate or missing evidence.',
    source: 'Assessment hierarchy',
  },
  {
    title: 'Make absence explicit',
    copy: 'Unavailable evidence is reported as Not reported, never silently coerced to zero.',
    source: 'Data integrity',
  },
  {
    title: 'Explain the direction',
    copy: 'Every baseline delta states whether the candidate improved, regressed, or stayed stable.',
    source: 'Comparison language',
  },
]

const motionCards = [
  {
    title: 'Objective outcome',
    copy: 'Hard gates and technical failures remain the release boundary.',
    status: 'hard_gate' as const,
    image: 'https://picsum.photos/seed/harness-outcome/1440/1080',
  },
  {
    title: 'Advisory conclusion',
    copy: 'AI interpretation is prominent, useful, and explicitly non-authoritative.',
    status: 'recommendation' as const,
    image: 'https://picsum.photos/seed/harness-advisory/1440/1080',
  },
  {
    title: 'Operational action',
    copy: 'The interface closes with a concrete investigation or comparison path.',
    status: 'passed' as const,
    image: 'https://picsum.photos/seed/harness-action/1440/1080',
  },
]

function ThemeControl() {
  const [theme, setTheme] = useState<'dark' | 'light'>(() =>
    document.documentElement.dataset.theme === 'light' ? 'light' : 'dark',
  )
  const next = theme === 'dark' ? 'light' : 'dark'
  const Icon = next === 'dark' ? Moon : Sun

  function updateTheme() {
    document.documentElement.dataset.theme = next
    document.documentElement.style.colorScheme = next
    localStorage.setItem('harness-e2e-theme', next)
    setTheme(next)
  }

  return (
    <Button
      variant="quiet"
      size="compact"
      aria-label={`Use ${next} theme`}
      onClick={updateTheme}
    >
      <Icon size={15} aria-hidden="true" />
      {next === 'dark' ? 'Dark' : 'Light'}
    </Button>
  )
}

export function DesignSystemPage() {
  const root = useRef<HTMLDivElement>(null)
  const [activeArchitecture, setActiveArchitecture] = useState('signal')
  const [guidanceIndex, setGuidanceIndex] = useState(0)

  useGSAP(
    () => {
      const media = gsap.matchMedia()

      media.add(
        '(min-width: 1024px) and (prefers-reduced-motion: no-preference)',
        () => {
          const stage = document.querySelector<HTMLElement>('.ds-motion-stage')
          const copy = document.querySelector<HTMLElement>('.ds-motion-copy')
          if (!stage || !copy) return
          ScrollTrigger.create({
            trigger: stage,
            start: 'top 96px',
            end: 'bottom bottom-=144',
            pin: copy,
            pinSpacing: false,
          })
        },
      )

      media.add('(prefers-reduced-motion: no-preference)', () => {
        for (const card of gsap.utils.toArray<HTMLElement>('.ds-motion-card')) {
          const image = card.querySelector<HTMLElement>('.ds-motion-image')
          if (!image) continue
          gsap
            .timeline({
              scrollTrigger: {
                trigger: card,
                start: 'top bottom',
                end: 'bottom top',
                scrub: true,
              },
            })
            .fromTo(
              image,
              {
                scale: 0.8,
                opacity: 0.3,
                filter: 'grayscale(1) contrast(1.25) brightness(0.55)',
              },
              {
                scale: 1,
                opacity: 1,
                filter: 'grayscale(1) contrast(1.25) brightness(1)',
                duration: 0.52,
              },
            )
            .to(image, {
              opacity: 0.2,
              filter: 'grayscale(1) contrast(1.25) brightness(0.42)',
              duration: 0.48,
            })
        }
      })

      return () => media.revert()
    },
    { scope: root },
  )

  const currentGuidance = guidance[guidanceIndex]

  return (
    <div className="ds-root ds-demo" ref={root}>
      <a className="ds-skip-link" href="#components">
        Skip to components
      </a>

      <nav className="ds-demo-nav" aria-label="Design system demonstration">
        <a className="ds-demo-brand" href="./">
          Harness <span>E2E</span>
        </a>
        <div className="ds-demo-nav-links">
          <a href="#components">Components</a>
          <a href="#motion-lab">Motion</a>
          <ThemeControl />
        </div>
      </nav>

      <main className="ds-demo-main">
        <header className="ds-demo-hero">
          <div className="ds-demo-hero-wash" aria-hidden="true" />
          <div className="ds-demo-hero-copy">
            <p>Harness E2E operational design system</p>
            <h1 className="ds-display">
              <span>Operational evidence.</span>
              <span>Decisive action.</span>
            </h1>
            <div className="ds-demo-hero-actions">
              <a
                className={buttonClassName({
                  variant: 'primary',
                  size: 'large',
                })}
                href="#components"
              >
                Explore primitives
              </a>
              <a
                className={buttonClassName({
                  variant: 'secondary',
                  size: 'large',
                })}
                href="#motion-lab"
              >
                Inspect motion
              </a>
            </div>
          </div>
        </header>

        <section className="ds-marquee" aria-label="Design system principles">
          <div className="ds-marquee-track">
            <div className="ds-marquee-row">
              <span>Objective first</span>
              <span>Evidence retained</span>
              <span>Advisory AI</span>
              <span>Explicit absence</span>
              <span>Keyboard ready</span>
            </div>
            <div className="ds-marquee-row" aria-hidden="true">
              <span>Objective first</span>
              <span>Evidence retained</span>
              <span>Advisory AI</span>
              <span>Explicit absence</span>
              <span>Keyboard ready</span>
            </div>
          </div>
        </section>

        <section className="ds-demo-section" id="components">
          <PageHeader
            context="Foundations and primitives"
            title="One visual language for every operational state"
            summary="Geist carries the product voice, Chivo Mono identifies retained evidence, and semantic colors preserve the boundary between outcome, uncertainty, and guidance."
            actions={
              <>
                <Button variant="primary">Run evaluation</Button>
                <Button>Compare evidence</Button>
              </>
            }
          />

          <div className="ds-bento">
            <MetricCard
              label="Scenario pass rate"
              value="92%"
              detail="Candidate improved without introducing a blocking regression."
              delta="+8 pp"
              tone="positive"
            />
            <MetricCard
              label="Hard gates"
              value="1"
              detail="A deterministic release boundary still requires attention."
              delta="Blocking"
              tone="warning"
            />
            <MetricCard
              label="Retained cost"
              value="Not reported"
              detail="Missing evidence remains unavailable and is never represented as zero."
              tone="unavailable"
            />

            <Panel className="ds-bento-wide" tone="spotlight">
              <PageHeader
                headingLevel={2}
                context="Operational semantics"
                title="Status is information, not decoration"
                summary="Each state retains a unique label, tone, and decision meaning across dashboard surfaces."
              />
              <div className="ds-status-gallery">
                <StatusBadge status="passed" />
                <StatusBadge status="failed" />
                <StatusBadge status="inconclusive" />
                <StatusBadge status="unavailable" />
                <StatusBadge status="hard_gate" />
                <StatusBadge status="recommendation" />
              </div>
            </Panel>

            <Panel className="ds-bento-narrow" tone="raised">
              <h2>Interaction hierarchy</h2>
              <p>
                Primary actions advance a workflow. Secondary actions inspect
                evidence. Quiet actions change local presentation.
              </p>
              <div className="ds-button-gallery">
                <Button variant="primary">Continue</Button>
                <Button>Inspect</Button>
                <Button variant="quiet">Dismiss</Button>
                <Button busy>Loading</Button>
              </div>
            </Panel>
          </div>
        </section>

        <section className="ds-demo-section ds-architecture-section">
          <PageHeader
            context="Information architecture"
            title="Dense information with a clear decision path"
            summary="The horizontal architecture explores the selected design direction without becoming a production component in phase one."
          />
          <div className="ds-horizontal-accordion">
            {architectureItems.map((item) => {
              const active = activeArchitecture === item.id
              return (
                <article
                  className={`ds-accordion-item${active ? ' is-active' : ''}`}
                  key={item.id}
                  style={
                    {
                      '--ds-accordion-image': `url(${item.image})`,
                    } as CSSProperties
                  }
                >
                  <button
                    type="button"
                    aria-expanded={active}
                    onClick={() => setActiveArchitecture(item.id)}
                  >
                    <span>{item.title}</span>
                    <strong>{item.copy}</strong>
                  </button>
                </article>
              )
            })}
          </div>
        </section>

        <section className="ds-demo-section ds-guidance-section">
          <Panel className="ds-guidance-card" tone="raised" padding="generous">
            <div className="ds-guidance-portrait" aria-hidden="true">
              <span>{String(guidanceIndex + 1).padStart(2, '0')}</span>
            </div>
            <div className="ds-guidance-copy" aria-live="polite">
              <StatusBadge status="recommendation" />
              <blockquote>{currentGuidance.copy}</blockquote>
              <div>
                <strong>{currentGuidance.title}</strong>
                <span>{currentGuidance.source}</span>
              </div>
            </div>
            <div className="ds-guidance-controls">
              <Button
                variant="quiet"
                size="compact"
                aria-label="Previous guidance"
                onClick={() =>
                  setGuidanceIndex(
                    (guidanceIndex - 1 + guidance.length) % guidance.length,
                  )
                }
              >
                <ChevronLeft size={17} aria-hidden="true" />
              </Button>
              <Button
                variant="quiet"
                size="compact"
                aria-label="Next guidance"
                onClick={() =>
                  setGuidanceIndex((guidanceIndex + 1) % guidance.length)
                }
              >
                <ChevronRight size={17} aria-hidden="true" />
              </Button>
            </div>
          </Panel>
        </section>

        <section className="ds-demo-section ds-motion-stage" id="motion-lab">
          <div className="ds-motion-copy">
            <p className="ds-page-context">Narrative comparison</p>
            <h2>Motion clarifies the evidence hierarchy.</h2>
            <p>
              Pinning is limited to wide screens. Image scale and fade track
              reading progress, while reduced-motion users receive the same
              content without scroll choreography.
            </p>
          </div>
          <div className="ds-motion-cards">
            {motionCards.map((card) => (
              <Panel
                className="ds-motion-card"
                key={card.title}
                padding="none"
                tone="raised"
              >
                <div className="ds-motion-media">
                  <img
                    className="ds-motion-image"
                    src={card.image}
                    alt="Abstract operational workspace"
                    loading="lazy"
                  />
                </div>
                <div className="ds-motion-card-copy">
                  <StatusBadge status={card.status} />
                  <h3>{card.title}</h3>
                  <p>{card.copy}</p>
                </div>
              </Panel>
            ))}
          </div>
        </section>

        <footer className="ds-demo-footer">
          <div>
            <p className="ds-page-context">Ready for controlled adoption</p>
            <h2>Foundations first. Production migration later.</h2>
          </div>
          <a
            className={buttonClassName({
              variant: 'primary',
              size: 'large',
            })}
            href="./"
          >
            Return to dashboard
          </a>
        </footer>
      </main>
    </div>
  )
}
