import * as React from 'react'
import { flexRender } from '@tanstack/react-table/flex-render'
import {
  getCoreRowModel,
  useLegacyTable,
  type LegacyColumnDef,
} from '@tanstack/react-table/legacy'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { cn } from '@/lib/utils'

export type { LegacyColumnDef }

export type DataTableProps<TData> = {
  columns: LegacyColumnDef<TData, unknown>[]
  data: TData[]
  className?: string
  emptyMessage?: React.ReactNode
  /** Receives the original row data (not a Row wrapper). */
  getRowId?: (row: TData) => string
}

export function DataTable<TData>({
  columns,
  data,
  className,
  emptyMessage = 'No rows.',
  getRowId,
}: DataTableProps<TData>) {
  const table = useLegacyTable({
    data,
    columns,
    getCoreRowModel: getCoreRowModel(),
    // TanStack passes the original row as the first argument — do not unwrap `.original`.
    getRowId: getRowId ? (originalRow: TData) => getRowId(originalRow) : undefined,
  })

  return (
    <div
      className={cn(
        'overflow-auto rounded-md border border-[color:var(--mc-border-soft)]',
        className,
      )}
    >
      <Table>
        <TableHeader>
          {table.getHeaderGroups().map((headerGroup) => (
            <TableRow key={headerGroup.id}>
              {headerGroup.headers.map((header) => (
                <TableHead key={header.id} className="whitespace-nowrap">
                  {header.isPlaceholder
                    ? null
                    : flexRender(header.column.columnDef.header, header.getContext())}
                </TableHead>
              ))}
            </TableRow>
          ))}
        </TableHeader>
        <TableBody>
          {table.getRowModel().rows.length === 0 ? (
            <TableRow>
              <TableCell colSpan={columns.length} className="h-16 text-center text-muted-foreground">
                {emptyMessage}
              </TableCell>
            </TableRow>
          ) : (
            table.getRowModel().rows.map((row) => (
              <TableRow key={row.id}>
                {row.getVisibleCells().map((cell) => (
                  <TableCell key={cell.id} className="align-top">
                    {flexRender(cell.column.columnDef.cell, cell.getContext())}
                  </TableCell>
                ))}
              </TableRow>
            ))
          )}
        </TableBody>
      </Table>
    </div>
  )
}
