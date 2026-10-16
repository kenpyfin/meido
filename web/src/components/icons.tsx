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

function MaidSvg({
  className,
  fallback,
  children,
}: {
  className?: string
  fallback: string
  children: React.ReactNode
}) {
  return (
    <svg
      className={base(className, fallback)}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      {children}
    </svg>
  )
}

/** Maid headdress (katyusha) with a centered bow. */
export function IconMeidoCrest({ className }: { className?: string }) {
  return (
    <MaidSvg className={className} fallback="size-6 shrink-0">
      <path d="M3.5 10.2C5.2 6.2 8.2 4.2 12 4.2s6.8 2 8.5 6" />
      <path d="M3 10.2h18" />
      <path d="M12 10.2c-1.15.1-2.35 1.05-2.7 2.45-.25.95.45 1.55 1.35 1.15L12 12.7" />
      <path d="M12 10.2c1.15.1 2.35 1.05 2.7 2.45.25.95-.45 1.55-1.35 1.15L12 12.7" />
      <path d="M11.15 13.35 9.1 17.4" />
      <path d="M12.85 13.35 14.9 17.4" />
      <circle cx="12" cy="8.15" r="0.7" fill="currentColor" stroke="none" />
    </MaidSvg>
  )
}

/** Collar ribbon, used on the active persona row. */
export function IconRibbonBow({ className }: { className?: string }) {
  return (
    <MaidSvg className={className} fallback="size-3.5 shrink-0">
      <path d="M12 10.5 8.2 7.6c-.7-.5-1.7.1-1.5.9.4 1.5 1.6 2.6 3.1 2.8" />
      <path d="M12 10.5 15.8 7.6c.7-.5 1.7.1 1.5.9-.4 1.5-1.6 2.6-3.1 2.8" />
      <path d="M10.6 12.2 8.4 17" />
      <path d="M13.4 12.2 15.6 17" />
      <circle cx="12" cy="11.2" r="0.9" fill="currentColor" stroke="none" />
    </MaidSvg>
  )
}

/** Parlor service bell for the inbox. */
export function IconServiceBell({ className }: { className?: string }) {
  return (
    <MaidSvg className={className} fallback="size-4 shrink-0">
      <path d="M12 4.5v1.2" />
      <path d="M6.2 15.2a6.2 6.2 0 0 1 11.6 0" />
      <path d="M5 15.2h14" />
      <path d="M9.2 15.4a2.8 2.8 0 0 0 5.6 0" />
      <path d="M8 18.6h8" />
    </MaidSvg>
  )
}

/** Teacup on a saucer, for the empty-thread welcome. */
export function IconTeacup({ className }: { className?: string }) {
  return (
    <MaidSvg className={className} fallback="size-4 shrink-0">
      <path d="M6 8.5h9.2v4.2a4.2 4.2 0 0 1-4.2 4.2H9.6A3.6 3.6 0 0 1 6 13.3Z" />
      <path d="M15.2 9.4h1.4a2 2 0 0 1 0 4h-1.4" />
      <path d="M5 19.2h14" />
    </MaidSvg>
  )
}

export type { LucideProps }
