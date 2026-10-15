import React, { createContext, useCallback, useContext, useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { AUTH_REQUIRED_EVENT, sanitizeHttpHeaderValue, setStoredAuthToken } from '../api/client'

type AuthContextValue = {
  authRequired: boolean
  authTokenInput: string
  setAuthTokenInput: (value: string) => void
  submitAuthToken: () => void
  setAuthRequired: (value: boolean) => void
}

const AuthContext = createContext<AuthContextValue | null>(null)

export function AuthProvider({
  children,
  onAuthError,
}: {
  children: React.ReactNode
  onAuthError: (message: string) => void
}) {
  const [authRequired, setAuthRequired] = useState(false)
  const [authTokenInput, setAuthTokenInput] = useState('')

  useEffect(() => {
    const onAuthRequired = () => setAuthRequired(true)
    window.addEventListener(AUTH_REQUIRED_EVENT, onAuthRequired)
    return () => window.removeEventListener(AUTH_REQUIRED_EVENT, onAuthRequired)
  }, [])

  const submitAuthToken = useCallback(() => {
    const token = sanitizeHttpHeaderValue(authTokenInput)
    if (!token) return
    if (token.length !== authTokenInput.trim().length) {
      onAuthError('Invalid API token: unsupported header characters.')
      return
    }
    setStoredAuthToken(token)
    setAuthRequired(false)
    setAuthTokenInput('')
    window.location.reload()
  }, [authTokenInput, onAuthError])

  const value: AuthContextValue = {
    authRequired,
    authTokenInput,
    setAuthTokenInput,
    submitAuthToken,
    setAuthRequired,
  }

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>
}

export function useAuth(): AuthContextValue {
  const ctx = useContext(AuthContext)
  if (!ctx) throw new Error('useAuth must be used within AuthProvider')
  return ctx
}

export function AuthDialog() {
  const { authRequired, authTokenInput, setAuthTokenInput, submitAuthToken, setAuthRequired } =
    useAuth()

  return (
    <Dialog open={authRequired} onOpenChange={(open) => !open && setAuthRequired(false)}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>API token required</DialogTitle>
          <DialogDescription>
            This server requires an API token. Use the same value as <code>WEB_AUTH_TOKEN</code> in
            your .env.
          </DialogDescription>
        </DialogHeader>
        <Input
          type="password"
          placeholder="Paste API token"
          value={authTokenInput}
          onChange={(e) => setAuthTokenInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') submitAuthToken()
          }}
        />
        <DialogFooter>
          <Button variant="outline" onClick={() => setAuthRequired(false)}>
            Cancel
          </Button>
          <Button onClick={submitAuthToken}>Save token</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
