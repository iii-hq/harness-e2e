import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { App } from '@/App'
import { installDashboardIiiClientFactory } from '@/lib/iii-client'
import { createStandaloneDashboardIiiClient } from '@/lib/standalone-iii-client'
import './index.css'

const root = document.querySelector<HTMLElement>('#root')
if (!root) throw new Error('missing #root container')

installDashboardIiiClientFactory(createStandaloneDashboardIiiClient)

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
