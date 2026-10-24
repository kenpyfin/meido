import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'

/** Sentinel for an explicit empty choice. Radix rejects an empty item value. */
const BLANK_MODEL = '__blank_model__'

export type ModelOption = {
  id: string
  label?: string
}

type Props = {
  value: string
  options: ModelOption[]
  onChange: (id: string) => void
  ariaLabel: string
  disabled?: boolean
  placeholder?: string
  className?: string
  /** Prepends a choice that stores an empty model id. */
  blankLabel?: string
  emptyHint?: string
}

export function listedModelIds(
  models: Array<{ id: string; from_active_config?: boolean }> | undefined,
): string[] {
  const ids: string[] = []
  for (const model of models ?? []) {
    if (!model.id || model.from_active_config) continue
    if (!ids.includes(model.id)) ids.push(model.id)
  }
  return ids
}

export function ModelSelect({
  value,
  options,
  onChange,
  ariaLabel,
  disabled,
  placeholder,
  className,
  blankLabel,
  emptyHint,
}: Props) {
  const known = options.filter((option) => option.id && option.id !== BLANK_MODEL)
  const inList = known.some((option) => option.id === value)
  const selectValue = inList ? value : value === '' && blankLabel ? BLANK_MODEL : undefined

  return (
    <div className="flex min-w-[220px] flex-1 flex-col gap-1">
      <Select
        value={selectValue}
        disabled={disabled || (known.length === 0 && !blankLabel)}
        onValueChange={(next) => onChange(next === BLANK_MODEL ? '' : next)}
      >
        <SelectTrigger aria-label={ariaLabel} className={className ?? 'w-full min-w-[220px]'}>
          <SelectValue
            placeholder={
              placeholder ?? (known.length === 0 ? 'No models loaded' : 'Select a model')
            }
          />
        </SelectTrigger>
        <SelectContent>
          {blankLabel ? <SelectItem value={BLANK_MODEL}>{blankLabel}</SelectItem> : null}
          {known.map((option) => (
            <SelectItem key={option.id} value={option.id}>
              {option.label?.trim() || option.id}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      {value && !inList ? (
        <span className="text-xs text-muted-foreground" role="status">
          Saved model is not in this list. Pick one.
        </span>
      ) : known.length === 0 ? (
        <span className="text-xs text-muted-foreground" role="status">
          {emptyHint ?? 'Refresh models, then pick one.'}
        </span>
      ) : null}
    </div>
  )
}
