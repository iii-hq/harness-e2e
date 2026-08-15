import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { ComparePage } from '@/pages/ComparePage'
import { CoveragePage } from '@/pages/CoveragePage'
import { ExecutionPage } from '@/pages/ExecutionPage'
import { OverviewPage } from '@/pages/OverviewPage'
import './index.css'

const root = document.querySelector<HTMLElement>('#root')
if (!root) throw new Error('missing #root container')

const pages = {
  compare: ComparePage,
  coverage: CoveragePage,
  execution: ExecutionPage,
  overview: OverviewPage,
}
const page = root.dataset.page as keyof typeof pages
const Page = pages[page] ?? OverviewPage

createRoot(root).render(
  <StrictMode>
    <Page />
  </StrictMode>,
)
