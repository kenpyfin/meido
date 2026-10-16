import React from 'react'

type MarkdownTableProps = React.TableHTMLAttributes<HTMLTableElement> & {
  className?: string
}

/** GFM table wrapper: horizontal scroll, sticky header, mobile-friendly cell wrapping. */
export function MarkdownTable({ className, ...props }: MarkdownTableProps) {
  return (
    <div className="mc-md-table-scroll">
      <table className={['aui-md-table', className].filter(Boolean).join(' ')} {...props} />
    </div>
  )
}
