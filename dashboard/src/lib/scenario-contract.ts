/**
 * The browser mirror of the harness `contract_fingerprint`. The Rust presenter
 * and `scripts/publish_harness_e2e_dashboard.py` hash the same object — the
 * case identity and the execution policy, never the scenario definition digest
 * — so a fingerprint computed here is comparable with a projected one.
 */

function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`
  if (value && typeof value === 'object') {
    return `{${Object.entries(value as Record<string, unknown>)
      .filter(([, item]) => item !== undefined)
      .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
      .map(([key, item]) => `${JSON.stringify(key)}:${canonical(item)}`)
      .join(',')}}`
  }
  return JSON.stringify(value ?? null)
}

/** FNV-1a over the canonical (sorted-key, compact) encoding of the contract. */
export function contractFingerprint(contract: unknown): string {
  const bytes = new TextEncoder().encode(canonical(contract))
  let hash = 2_166_136_261
  for (const byte of bytes) {
    hash = Math.imul(hash ^ byte, 16_777_619) >>> 0
  }
  return `fnv1a32:${hash.toString(16).padStart(8, '0')}`
}

/** The contract a retained scenario carries: case identity plus its policy. */
export function scenarioContractFingerprint(scenario: {
  scenario_id?: unknown
  case_id?: unknown
  case?: unknown
  execution_policy?: unknown
}): string {
  const caseId =
    typeof scenario.case_id === 'string' && scenario.case_id.length > 0
      ? scenario.case_id
      : null
  const contract: Record<string, unknown> = {
    case_id: caseId,
    execution_policy: scenario.execution_policy ?? {},
    scenario_id: scenario.scenario_id,
  }
  if (
    scenario.case &&
    typeof scenario.case === 'object' &&
    !Array.isArray(scenario.case)
  ) {
    contract.case = scenario.case
  }
  return contractFingerprint(contract)
}
