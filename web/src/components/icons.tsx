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

/** Traced headdress. Porcelain, claret, and brass live in the file. Themes do not recolor it. */
export function IconMeidoCrest({ className }: { className?: string }) {
  return (
    <img
      src="/meido-crest.svg"
      alt=""
      draggable={false}
      className={base(className, 'shrink-0 object-contain')}
    />
  )
}

/** Bow taken from the crest. Espresso comes from currentColor on the ribbon. */
export function IconSendBow({ className }: { className?: string }) {
  return (
    <svg
      className={base(className, 'size-3.5 shrink-0')}
      viewBox="0 0 160 129"
      fill="currentColor"
      aria-hidden
    >
      <path
        transform="translate(36 4)"
        d="M0 0 C13.5 3.7 24 11.5 34 21 C34.1 20.8 34.1 20.8 34.8 19.5 C36 18 36 18 38.6 17.5 C39.4 17.3 40.2 17.2 41 17 C41.2 16.5 41.2 16.5 42 14 C44.9 14.6 45.9 14.9 48 17 C48.7 17.3 49.4 17.6 50.1 17.9 C50.7 18.3 51.4 18.6 52 19 C52 20 52 21 52 22 C52.7 21.4 53.4 20.9 54.1 20.3 C78.4 0 78.4 0 93 -0.2 C93.8 -0.3 94.7 -0.3 95.6 -0.3 C98.6 0.1 99.8 0.9 102 3 C102.7 5.2 102.7 5.2 103.1 7.7 C103.2 8.6 103.4 9.5 103.5 10.4 C103.6 10.9 103.6 10.9 104 13.3 C106.1 23.9 106.1 23.9 110 26.6 C112.8 28.2 115.9 29.1 119 30 C122.2 36.4 119 44.4 117.1 50.8 C117 51.2 117 51.2 116.3 53.6 C113.8 62 113.8 62 112 65 C104.9 67.5 96.8 64.2 90.3 61.2 C88.8 60.5 87.4 59.7 86 59 C86.6 59.9 87.1 60.8 87.7 61.8 C88.5 63 89.2 64.3 90 65.6 C90.4 66.2 90.8 66.9 91.2 67.6 C94 72.2 96.5 76.9 98.9 81.6 C102.3 88.2 106.5 93.9 111.1 99.6 C113 102 113 102 113 103 C96.9 103.4 96.9 103.4 89 101 C88.9 101.5 88.9 102.1 88.8 102.7 C87.7 109 85.3 114.1 81 119 C79.7 119.7 78.4 120.4 77 121 C75.1 117.6 73.2 114.1 71.3 110.7 C70.8 109.7 70.3 108.8 69.7 107.8 C61.4 92.6 56 78 53.8 60.8 C53.6 59 53.3 57.2 53.1 55.4 C52.8 53.7 52.6 52.1 52.4 50.5 C52 48 52 48 51 46 C48.5 45.6 48.5 45.6 46 46 C45.2 48 45.2 48 45 50 C44.5 49.8 44.5 49.8 42 49 C42 48 42 47 42 46 C39.6 45.6 39.6 45.6 37 46 C34.6 51.2 34.3 56.8 33.7 62.4 C31.3 81.4 24.5 98.1 14.7 114.6 C14.4 115.1 14 115.7 13.7 116.2 C12.8 117.8 12.8 117.8 11 120 C10 120 9 120 8 120 C3.7 115.6 -0.9 109.1 -1.1 102.8 C-1 102.2 -1 101.6 -1 101 C-1.6 101.1 -2.1 101.3 -2.7 101.4 C-10.2 103.2 -17.4 103.2 -25 103 C-23.2 100.3 -21.4 97.5 -19.5 94.9 C-15.1 88.9 -11.4 82.7 -8 76.2 C-7.1 74.5 -6.2 72.9 -5.4 71.3 C-1.2 63.5 -1.2 63.5 0 60 C-0.8 60.3 -1.5 60.7 -2.3 61.1 C-16.5 67.3 -16.5 67.3 -23.1 65.4 C-25.9 63.4 -26.2 61.3 -27.1 58.1 C-27.2 57.5 -27.4 56.9 -27.6 56.3 C-27.9 55.1 -28.2 53.9 -28.5 52.7 C-29 51.1 -29.4 49.6 -29.9 48 C-33.6 35.9 -33.6 35.9 -32 31 C-29 28 -25.5 26.3 -21.4 25.5 C-18.4 23.6 -18.4 22.7 -17.6 19.3 C-17.2 17.4 -16.8 15.5 -16.4 13.6 C-16.1 11.6 -15.7 9.7 -15.3 7.8 C-15.2 7 -15 6.1 -14.8 5.2 C-12.4 -1.4 -5.9 -0.8 0 0 Z"
      />
    </svg>
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

export type { LucideProps }
