import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { DesignSystemPage } from './DesignSystemPage'
import './styles.css'
import './demo.css'

const root = document.querySelector<HTMLElement>('#root')
if (!root) throw new Error('missing #root container')

createRoot(root).render(
  <StrictMode>
    <DesignSystemPage />
  </StrictMode>,
)
