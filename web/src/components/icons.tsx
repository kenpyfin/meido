import * as React from 'react'
import {
  Star,
  ChevronDown,
  ChevronUp,
  Settings,
  Copy,
  Reply,
  Trash2,
  Pencil,
  MessageSquareText,
  MoreVertical,
  Gauge,
  PanelLeft,
  LayoutGrid,
  Inbox,
  ListTree,
  Split,
  type LucideProps,
} from 'lucide-react'
import { cn } from '@/lib/utils'

type IconProps = { className?: string; filled?: boolean }

const base = (className: string | undefined, fallback: string) => cn(fallback, className)

export function IconStar({ className, filled }: IconProps) {
  return (
    <Star
      className={base(className, 'size-4 shrink-0')}
      fill={filled ? 'currentColor' : 'none'}
      aria-hidden
    />
  )
}

export function IconChevronDown({ className }: { className?: string }) {
  return <ChevronDown className={base(className, 'size-3 shrink-0')} aria-hidden />
}

export function IconChevronUp({ className }: { className?: string }) {
  return <ChevronUp className={base(className, 'size-3 shrink-0')} aria-hidden />
}

export function IconSettings({ className }: { className?: string }) {
  return <Settings className={base(className, 'size-5 shrink-0')} aria-hidden />
}

export function IconCopy({ className }: { className?: string }) {
  return <Copy className={base(className, 'size-4 shrink-0')} aria-hidden />
}

export function IconReply({ className }: { className?: string }) {
  return <Reply className={base(className, 'size-4 shrink-0')} aria-hidden />
}

export function IconTrash({ className }: { className?: string }) {
  return <Trash2 className={base(className, 'size-4 shrink-0')} aria-hidden />
}

export function IconPencil({ className }: { className?: string }) {
  return <Pencil className={base(className, 'size-4 shrink-0')} aria-hidden />
}

export function IconSideChat({ className }: { className?: string }) {
  return <MessageSquareText className={base(className, 'size-4 shrink-0')} aria-hidden />
}

export function IconMoreVertical({ className }: { className?: string }) {
  return <MoreVertical className={base(className, 'size-4 shrink-0')} aria-hidden />
}

export function IconCockpit({ className }: { className?: string }) {
  return <Gauge className={base(className, 'size-5 shrink-0')} aria-hidden />
}

export function IconSidebar({ className }: { className?: string }) {
  return <PanelLeft className={base(className, 'size-5 shrink-0')} aria-hidden />
}

export function IconOps({ className }: { className?: string }) {
  return <LayoutGrid className={base(className, 'size-5 shrink-0')} aria-hidden />
}

export function IconInbox({ className }: { className?: string }) {
  return <Inbox className={base(className, 'size-5 shrink-0')} aria-hidden />
}

export function IconListTree({ className }: { className?: string }) {
  return <ListTree className={base(className, 'size-4 shrink-0')} aria-hidden />
}

export function IconSplit({ className }: { className?: string }) {
  return <Split className={base(className, 'size-4 shrink-0')} aria-hidden />
}

export type { LucideProps }
