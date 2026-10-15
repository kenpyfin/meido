import { useRef } from 'react'
import { QueryClientProvider } from '@tanstack/react-query'
import { createRoot } from 'react-dom/client'
import { App } from './app/App'
import { AuthProvider } from './context/AuthContext'
import { queryClient } from './query-client'

function Root() {
  const onAuthErrorRef = useRef<(message: string) => void>(() => {})

  return (
    <QueryClientProvider client={queryClient}>
      <AuthProvider onAuthError={(message) => onAuthErrorRef.current(message)}>
        <App onAuthErrorRef={onAuthErrorRef} />
      </AuthProvider>
    </QueryClientProvider>
  )
}

createRoot(document.getElementById('root')!).render(<Root />)
