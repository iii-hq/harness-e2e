/// <reference types="vite/client" />

export {}

declare global {
  interface Window {
    BENCHMARK_DATA?: unknown
    HARNESS_BENCHMARK_PREVIEW?: boolean
    __HARNESS_REACT_BOOT__?: Partial<Record<LegacyPageName, Promise<void>>>
  }
}

type LegacyPageName = 'coverage'
