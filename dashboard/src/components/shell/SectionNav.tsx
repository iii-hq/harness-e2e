import {
  BottomSheet,
  BottomSheetContent,
  BottomSheetTitle,
  BottomSheetTrigger,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from '@iii-dev/console-ui'
import { ChevronDown } from 'lucide-react'
import { useState } from 'react'

const SHEET_LIST = {
  display: 'grid',
  gap: 4,
  margin: 0,
  padding: '8px 0 0',
  listStyle: 'none',
} as const

const SHEET_ROW = {
  display: 'flex',
  alignItems: 'center',
  minHeight: 48,
  padding: '0 16px',
  borderRadius: 6,
  color: 'var(--color-ink)',
  fontSize: 16,
  textDecoration: 'none',
} as const

export type SectionLink<Section extends string> = {
  value: Section
  label: string
  href: string
}

/** The Harness E2E sections in the Console header: links with aria-current
 *  on a wide pane, a menu below 720 px, a "Sections" sheet on a phone. */
export function SectionNav<Section extends string>({
  sections,
  current,
  narrow,
  phone,
  onNavigate,
}: {
  sections: SectionLink<Section>[]
  current: Section
  narrow: boolean
  phone: boolean
  onNavigate: (section: Section) => void
}) {
  const [sheetOpen, setSheetOpen] = useState(false)
  const currentLabel =
    sections.find((section) => section.value === current)?.label ?? ''

  if (phone) {
    return (
      <nav className="harness-e2e-sections" aria-label="Harness E2E sections">
        <BottomSheet open={sheetOpen} onOpenChange={setSheetOpen}>
          <BottomSheetTrigger asChild>
            <button
              type="button"
              className="harness-e2e-section-menu"
              aria-label={`Section: ${currentLabel}`}
            >
              {currentLabel}
              <ChevronDown size={16} aria-hidden="true" />
            </button>
          </BottomSheetTrigger>
          <BottomSheetContent>
            <BottomSheetTitle>Sections</BottomSheetTitle>
            {/* The host portals the sheet out of our scoped stylesheet, so
                its 48 px rows are styled inline. */}
            <ul style={SHEET_LIST}>
              {sections.map((section) => (
                <li key={section.value}>
                  <a
                    style={{
                      ...SHEET_ROW,
                      fontWeight: section.value === current ? 600 : 400,
                      background:
                        section.value === current
                          ? 'var(--color-surface-selected)'
                          : 'transparent',
                    }}
                    href={section.href}
                    aria-current={
                      section.value === current ? 'page' : undefined
                    }
                    onClick={() => setSheetOpen(false)}
                  >
                    {section.label}
                  </a>
                </li>
              ))}
            </ul>
          </BottomSheetContent>
        </BottomSheet>
      </nav>
    )
  }

  if (narrow) {
    return (
      <nav className="harness-e2e-sections" aria-label="Harness E2E sections">
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              className="harness-e2e-section-menu"
              aria-label={`Section: ${currentLabel}`}
            >
              {currentLabel}
              <ChevronDown size={16} aria-hidden="true" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start">
            <DropdownMenuRadioGroup
              value={current}
              onValueChange={(value) => onNavigate(value as Section)}
            >
              {sections.map((section) => (
                <DropdownMenuRadioItem
                  key={section.value}
                  value={section.value}
                >
                  {section.label}
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      </nav>
    )
  }

  return (
    <nav className="harness-e2e-sections" aria-label="Harness E2E sections">
      <ul className="harness-e2e-section-tabs">
        {sections.map((section) => (
          <li key={section.value}>
            <a
              className="harness-e2e-section-tab"
              href={section.href}
              aria-current={section.value === current ? 'page' : undefined}
            >
              {section.label}
            </a>
          </li>
        ))}
      </ul>
    </nav>
  )
}
