import { createContext, type ReactNode, useContext, useMemo } from 'react'

type ScenarioChatContextValue = {
  openChat: ((sessionId: string) => void) | null
}

const ScenarioChatContext = createContext<ScenarioChatContextValue>({
  openChat: null,
})

export function ScenarioChatProvider({
  openChat,
  children,
}: {
  openChat?: (sessionId: string) => void
  children: ReactNode
}) {
  const value = useMemo(() => ({ openChat: openChat ?? null }), [openChat])
  return (
    <ScenarioChatContext.Provider value={value}>
      {children}
    </ScenarioChatContext.Provider>
  )
}

export function useScenarioChat() {
  return useContext(ScenarioChatContext)
}
