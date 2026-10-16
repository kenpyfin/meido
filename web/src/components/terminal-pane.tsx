import React, { useCallback, useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { xtermThemeFromCss } from '@/lib/xterm-theme'
import { FitAddon } from '@xterm/addon-fit'
import { WebLinksAddon } from '@xterm/addon-web-links'
import { Terminal } from '@xterm/xterm'
import '@xterm/xterm/css/xterm.css'
import { api } from '../api/client'

export type { TerminalCapabilities } from '../types'

type TerminalSessionResponse = {
  ok?: boolean
  session_id?: string
  ws_ticket?: string
  cwd?: string
  expires_in_secs?: number
}

type ConnectionState = 'idle' | 'connecting' | 'connected' | 'error' | 'closed'

export type TerminalPaneProps = {
  active: boolean
  onError?: (message: string) => void
}

function terminalWsUrl(): string {
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${protocol}//${window.location.host}/api/terminal/ws`
}

export function TerminalPane({ active, onError }: TerminalPaneProps) {
  const containerRef = useRef<HTMLDivElement | null>(null)
  const termRef = useRef<Terminal | null>(null)
  const fitRef = useRef<FitAddon | null>(null)
  const wsRef = useRef<WebSocket | null>(null)
  const sessionRef = useRef<{ sessionId: string; cwd: string } | null>(null)
  const [connectionState, setConnectionState] = useState<ConnectionState>('idle')
  const [statusMessage, setStatusMessage] = useState<string>('')

  const cleanup = useCallback(() => {
    const ws = wsRef.current
    wsRef.current = null
    if (ws && (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)) {
      ws.close()
    }
    const term = termRef.current
    if (term) {
      term.dispose()
      termRef.current = null
      fitRef.current = null
    }
  }, [])

  const sendResize = useCallback(() => {
    const term = termRef.current
    const ws = wsRef.current
    if (!term || !ws || ws.readyState !== WebSocket.OPEN) return
    const payload = JSON.stringify({
      type: 'resize',
      cols: term.cols,
      rows: term.rows,
    })
    ws.send(payload)
  }, [])

  const connect = useCallback(async () => {
    cleanup()
    setConnectionState('connecting')
    setStatusMessage('Starting session…')

    try {
      const session = await api<TerminalSessionResponse>('/api/terminal/sessions', {
        method: 'POST',
      })
      const sessionId = session.session_id
      const ticket = session.ws_ticket
      const cwd = session.cwd ?? ''
      if (!sessionId || !ticket) {
        throw new Error('Invalid terminal session response')
      }
      sessionRef.current = { sessionId, cwd }

      const container = containerRef.current
      if (!container) {
        throw new Error('Terminal container unavailable')
      }

      const xtermTheme = xtermThemeFromCss()
      const term = new Terminal({
        cursorBlink: true,
        fontFamily: '"JetBrains Mono", ui-monospace, monospace',
        fontSize: 13,
        theme: xtermTheme,
        allowProposedApi: true,
      })
      const fitAddon = new FitAddon()
      term.loadAddon(fitAddon)
      term.loadAddon(new WebLinksAddon())
      term.open(container)
      fitAddon.fit()
      termRef.current = term
      fitRef.current = fitAddon

      const ws = new WebSocket(terminalWsUrl())
      ws.binaryType = 'arraybuffer'
      wsRef.current = ws

      ws.onopen = () => {
        ws.send(
          JSON.stringify({
            type: 'auth',
            session_id: sessionId,
            ticket,
          }),
        )
      }

      ws.onmessage = (event) => {
        if (typeof event.data === 'string') {
          try {
            const msg = JSON.parse(event.data) as {
              type?: string
              message?: string
              cwd?: string
              code?: number
            }
            if (msg.type === 'auth_ok') {
              setConnectionState('connected')
              setStatusMessage(msg.cwd ? `cwd: ${msg.cwd}` : cwd ? `cwd: ${cwd}` : 'Connected')
              sendResize()
              term.focus()
              return
            }
            if (msg.type === 'exit') {
              setConnectionState('closed')
              setStatusMessage(
                typeof msg.code === 'number' ? `Shell exited (${msg.code})` : 'Shell exited',
              )
              return
            }
            if (msg.type === 'error') {
              const message = msg.message || 'Terminal error'
              setConnectionState('error')
              setStatusMessage(message)
              onError?.(message)
              term.writeln(`\r\n\x1b[31m${message}\x1b[0m`)
              return
            }
          } catch {
            term.write(event.data)
          }
          return
        }

        if (event.data instanceof ArrayBuffer) {
          term.write(new Uint8Array(event.data))
        } else if (event.data instanceof Blob) {
          void event.data.arrayBuffer().then((buf) => {
            term.write(new Uint8Array(buf))
          })
        }
      }

      ws.onerror = () => {
        setConnectionState('error')
        setStatusMessage('WebSocket connection failed')
        onError?.('WebSocket connection failed')
      }

      ws.onclose = () => {
        setConnectionState((prev) => {
          if (prev === 'connecting') {
            setStatusMessage('Connection closed')
          }
          return prev === 'connected' ? 'closed' : prev
        })
      }

      term.onData((data) => {
        const socket = wsRef.current
        if (!socket || socket.readyState !== WebSocket.OPEN) return
        socket.send(new TextEncoder().encode(data))
      })
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err)
      setConnectionState('error')
      setStatusMessage(message)
      onError?.(message)
    }
  }, [cleanup, onError, sendResize])

  const disconnect = useCallback(() => {
    cleanup()
    setConnectionState('closed')
    setStatusMessage('Disconnected')
  }, [cleanup])

  useEffect(() => {
    if (!active) {
      cleanup()
      setConnectionState('idle')
      setStatusMessage('')
      return undefined
    }

    void connect()

    const onResize = () => {
      fitRef.current?.fit()
      sendResize()
    }
    window.addEventListener('resize', onResize)

    return () => {
      window.removeEventListener('resize', onResize)
      cleanup()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- connect once per dialog open
  }, [active])

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="font-mono text-xs text-muted-foreground">
          {sessionRef.current?.sessionId
            ? `session ${sessionRef.current.sessionId.slice(0, 8)}`
            : 'no session'}
          {statusMessage ? ` · ${statusMessage}` : ''}
        </span>
        <div className="flex gap-2">
          <Button
            size="sm"
            variant="secondary"
            disabled={connectionState === 'connecting'}
            onClick={() => void connect()}
          >
            Reconnect
          </Button>
          <Button
            size="sm"
            variant="destructive"
            disabled={connectionState === 'idle' || connectionState === 'closed'}
            onClick={disconnect}
          >
            Disconnect
          </Button>
        </div>
      </div>
      <div
        ref={containerRef}
        className="min-h-[min(60vh,480px)] flex-1 overflow-hidden rounded-md border border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-main)] p-1"
        aria-label="Interactive terminal"
      />
      <p className="text-xs text-muted-foreground">
        State: {connectionState}. Requires WEB_AUTH_TOKEN and WEB_TERMINAL_ENABLED on the gateway host.
      </p>
    </div>
  )
}
