import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { App } from '@/App'
import { installDashboardIiiClientFactory } from '@/lib/iii-client'
import { createStandaloneDashboardIiiClient } from '@/lib/standalone-iii-client'
import './index.css'

const root = document.querySelector<HTMLElement>('#root')
if (!root) throw new Error('missing #root container')

installDashboardIiiClientFactory(createStandaloneDashboardIiiClient)
// The document root carries the shell tokens in standalone mode (see
// dashboard-shell.css); index.html sets it statically, this keeps it true
// when the bundle is mounted elsewhere.
document.documentElement.dataset.harnessE2e = 'standalone'

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
