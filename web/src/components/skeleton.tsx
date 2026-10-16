import React from 'react'

type SkeletonProps = {
  className?: string
  style?: React.CSSProperties
}

export function Skeleton({ className, style }: SkeletonProps) {
  return <div className={`mc-skeleton ${className ?? ''}`.trim()} style={style} aria-hidden />
}

export function SettingsPanelSkeleton() {
  return (
    <div className="flex flex-col gap-3" aria-busy="true" aria-label="Loading settings">
      <Skeleton className="h-4 w-3/4 max-w-md" />
      <Skeleton className="h-10 w-full" />
      <Skeleton className="h-10 w-full" />
      <Skeleton className="h-8 w-32" />
    </div>
  )
}

export function OverviewStatusSkeleton() {
  return (
    <div className="mb-2 flex flex-col gap-2" aria-busy="true" aria-label="Loading installation status">
      <div className="flex flex-wrap gap-2">
        <Skeleton className="h-4 w-20" />
        <Skeleton className="h-4 w-24" />
        <Skeleton className="h-4 w-32" />
      </div>
      <Skeleton className="h-4 w-full max-w-lg" />
      <Skeleton className="h-8 w-36" />
    </div>
  )
}

export function ArtifactListSkeleton() {
  return (
    <div className="flex flex-col gap-2 p-2" aria-busy="true" aria-label="Loading artifacts">
      <Skeleton className="h-10 w-full" />
      <Skeleton className="h-10 w-full" />
      <Skeleton className="h-10 w-full" />
      <Skeleton className="h-10 w-4/5" />
    </div>
  )
}

export function ContentPreviewSkeleton() {
  return (
    <div className="flex flex-col gap-2 py-2" aria-busy="true" aria-label="Loading preview">
      <Skeleton className="h-4 w-full" />
      <Skeleton className="h-4 w-full" />
      <Skeleton className="h-4 w-3/4" />
    </div>
  )
}

export function ThreadHistorySkeleton() {
  return (
    <div
      className="mc-thread-skeleton flex min-h-0 flex-1 flex-col gap-4 overflow-hidden px-3 py-6"
      aria-busy="true"
      aria-label="Loading conversation"
    >
      <div className="flex justify-end">
        <Skeleton className="mc-thread-skeleton-bubble mc-thread-skeleton-user h-12 w-[min(72%,280px)]" />
      </div>
      <div className="flex justify-start">
        <Skeleton className="mc-thread-skeleton-bubble h-20 w-[min(80%,360px)]" />
      </div>
      <div className="flex justify-end">
        <Skeleton className="mc-thread-skeleton-bubble mc-thread-skeleton-user h-10 w-[min(55%,200px)]" />
      </div>
      <div className="flex justify-start">
        <Skeleton className="mc-thread-skeleton-bubble h-16 w-[min(75%,320px)]" />
      </div>
    </div>
  )
}
