/// <reference types="vite/client" />

interface Window {
  BENCHMARK_DATA?: unknown
  HARNESS_BENCHMARK_PREVIEW?: boolean
  HARNESS_EXECUTIONS?: { mode?: string } & Record<string, unknown>
  HARNESS_EXECUTION_DETAILS?: Record<string, unknown>
  HARNESS_COVERAGE?: unknown
  HarnessAnsiLog?: unknown
  HarnessBenchmarkData?: unknown
  HarnessExecutionData?: unknown
  HarnessExecutionTranscript?: unknown
  HarnessLocalRunner?: unknown
  __HARNESS_REACT_BOOT__?: Partial<Record<LegacyPageName, Promise<void>>>
}

type LegacyPageName = 'overview' | 'execution' | 'compare' | 'coverage'
