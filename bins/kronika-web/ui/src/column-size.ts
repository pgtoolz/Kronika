export function fittedWidth(widest: number): number {
  return Math.min(MAX_COLUMN, Math.max(MIN_COLUMN, Math.ceil(widest) + GRIP_ROOM))
}

const MIN_COLUMN = 64
const MAX_COLUMN = 720
const GRIP_ROOM = 12

export function widestCell(table: HTMLElement, index: number): number {
  const cells: HTMLElement[] = []
  for (const row of table.querySelectorAll<HTMLElement>("[role=\"row\"]")) {
    const cell = row.children[index]
    if (cell instanceof HTMLElement) cells.push(cell)
  }
  return measure(table, cells).reduce((most, width) => Math.max(most, width), 0)
}

export function headerWidths(head: HTMLElement): readonly number[] {
  return measure(head, [...head.children].filter((cell): cell is HTMLElement => cell instanceof HTMLElement))
    .map((width) => Math.ceil(width) + GRIP_ROOM)
}

function measure(anchor: HTMLElement, cells: readonly HTMLElement[]): readonly number[] {
  const document_ = anchor.ownerDocument
  const shadow = document_.createElement("div")
  shadow.style.cssText = "position:absolute;left:-9999px;top:0;visibility:hidden"
  const copies = cells.map((cell) => {
    const copy = cell.cloneNode(true) as HTMLElement
    unbind(copy)
    for (const nested of copy.querySelectorAll<HTMLElement>("*")) unbind(nested)
    shadow.append(copy)
    return copy
  })
  document_.body.append(shadow)
  const widths = copies.map((copy) => copy.getBoundingClientRect().width)
  shadow.remove()
  return widths
}

function unbind(element: HTMLElement): void {
  element.style.width = "max-content"
  element.style.maxWidth = "none"
  element.style.minWidth = "0"
  element.style.overflow = "visible"
  element.style.position = "static"
}
