/// <reference types="vite/client" />

import type { DashboardRoutes } from '@/hooks/use-hash-route'
import type {
  DashboardDataBridge,
  ExecutionManifest,
} from '@/lib/dashboard-data-source'

declare global {
  interface Window {
    BENCHMARK_DATA?: unknown
    HARNESS_BENCHMARK_PREVIEW?: boolean
    HARNESS_EXECUTIONS?: ExecutionManifest
    HARNESS_EXECUTION_DETAILS?: Record<string, unknown>
    HARNESS_COVERAGE?: unknown
    HarnessAnsiLog?: unknown
    HarnessBenchmarkData?: unknown
    HarnessDashboardData?: DashboardDataBridge
    HarnessDashboardRoutes: DashboardRoutes
    HarnessExecutionData?: unknown
    HarnessExecutionTranscript?: unknown
    HarnessLocalRunner?: unknown
    __HARNESS_REACT_BOOT__?: Partial<Record<LegacyPageName, Promise<void>>>
  }
}

type LegacyPageName = 'overview' | 'execution' | 'coverage'
