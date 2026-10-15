import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { ENGINE_OPTIONS, asEngineId, type EngineId } from '../lib/agent-engine'

type Props = {
  value: EngineId
  onChange: (engine: EngineId) => void
  disabled?: boolean
  id?: string
  /** Compact trigger for the conversation composer. */
  size?: 'sm' | 'default'
}

export function AgentEngineSelect({
  value,
  onChange,
  disabled = false,
  id,
  size = 'default',
}: Props) {
  return (
    <Select
      value={value}
      disabled={disabled}
      onValueChange={(next) => onChange(asEngineId(next))}
    >
      <SelectTrigger id={id} size={size} aria-label="Agent engine" className="max-w-[240px]">
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {ENGINE_OPTIONS.map((opt) => (
          <SelectItem key={opt.id} value={opt.id} title={opt.subtitle}>
            {opt.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}
