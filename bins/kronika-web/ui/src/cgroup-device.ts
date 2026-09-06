import type { Cell, DataRow } from "./api"
import { rawText, value } from "./model"

// io.stat charges every traversed block layer; only recorded topology edges
// allow a lower layer to fold into another device in the same cgroup.

export interface CgroupMountAssociation {
  readonly infrastructure: boolean | null
  readonly mountPoint: string
  readonly root: string | null
  readonly source: string | null
  // The mounted device when it is a layer above the charged one.
  readonly via: string | null
}

export interface CgroupDeviceLayer {
  readonly id: string
  readonly name: string | null
}

export interface CgroupDevicePresentation {
  readonly associations: readonly CgroupMountAssociation[]
  // The exact chain from the mounted layer down to the physical device; the
  // charged device is one of its layers.
  readonly chain: readonly CgroupDeviceLayer[]
  readonly device: string | null
  // The charged device above this one in the same cgroup that owns the table
  // row, when this one is a lower layer of the same I/O.
  readonly foldedInto: string | null
  readonly id: string
  readonly preferredMounts: readonly string[]
  readonly source: string | null
}

// child major:minor → the devices directly beneath it, sorted.
export type BlockParents = ReadonlyMap<string, readonly string[]>

const NO_PARENTS: BlockParents = new Map()

export function blockParents(edges: readonly DataRow[]): BlockParents {
  const parents = new Map<string, string[]>()
  for (const edge of edges) {
    const child = exactDeviceId(edge)
    const major = rawText(value(edge, "parent_major"))
    const minor = rawText(value(edge, "parent_minor"))
    if (child === null || major === null || minor === null) continue
    const stored = parents.get(child) ?? []
    const parent = `${major}:${minor}`
    if (!stored.includes(parent)) stored.push(parent)
    parents.set(child, stored)
  }
  for (const stored of parents.values()) stored.sort(compareText)
  return parents
}

export function cgroupDevicePresentation(
  row: DataRow,
  mounts: readonly DataRow[],
  devices: readonly DataRow[],
  parents: BlockParents = NO_PARENTS,
  charged: readonly string[] = [],
): CgroupDevicePresentation | null {
  const id = exactDeviceId(row)
  if (id === null) return null
  const names = deviceNames(devices)
  const children = childIndex(parents)
  const above = reachable(id, (device) => children.get(device) ?? [])
  const chargedAbove = charged.filter((other) => other !== id && above.includes(other))
  const foldedInto = chargedAbove.find((candidate) => !chargedAbove.some((other) => other !== candidate && reachable(candidate, (device) => children.get(device) ?? []).includes(other)))
    ?? chargedAbove[0] ?? null
  // A charged layer above owns its own mounts; the others lend theirs.
  const lenders = above.filter((device) => !charged.includes(device))
  const associations = [
    ...mounts.filter((mount) => exactDeviceId(mount) === id).map((mount) => association(mount, null)),
    ...lenders.flatMap((device) => mounts.filter((mount) => exactDeviceId(mount) === device).map((mount) => association(mount, device))),
  ].filter((candidate): candidate is CgroupMountAssociation => candidate !== null)
    .filter((candidate, index, all) => all.findIndex((other) => associationKey(other) === associationKey(candidate)) === index)
    .sort(compareAssociations)
  const preferred = associations.filter(({ infrastructure }) => infrastructure !== true)
  const top = preferred[0]?.via ?? id
  const chain = [...route(top, id, (device) => parents.get(device) ?? []), ...descend(id, parents).slice(1)]
  return {
    associations,
    chain: chain.map((layer) => ({ id: layer, name: names.get(layer) ?? null })),
    device: names.get(id) ?? null,
    foldedInto,
    id,
    preferredMounts: [...new Set(preferred.map(({ mountPoint }) => mountPoint))],
    source: preferred.find(({ source }) => source !== null)?.source ?? associations.find(({ source }) => source !== null)?.source ?? null,
  }
}

export function cgroupDevicePrimary(presentation: CgroupDevicePresentation): string {
  return presentation.preferredMounts[0] ?? presentation.device ?? presentation.id
}

// The chain under the primary line: the mount source when it names the top
// layer differently, then the named layers down to the physical device.
// Unnamed layers between named ones are left to the Inspector chain.
export function cgroupDeviceSecondary(presentation: CgroupDevicePresentation): string | null {
  const last = presentation.chain.length - 1
  const layers = presentation.chain
    .filter((layer, index) => index === 0 || index === last || layer.name !== null)
    .map((layer) => layer.name ?? layer.id)
  const base = sourceBasename(presentation.source)
  const first = layers[0]
  if (first !== undefined && base !== null && presentation.preferredMounts.length > 0 && base !== first) layers[0] = `${base} · ${first}`
  const text = layers.join(" → ")
  return text === "" || text === cgroupDevicePrimary(presentation) ? null : text
}

export function cgroupDeviceChain(presentation: CgroupDevicePresentation): string {
  return presentation.chain.map((layer) => layer.name === null ? layer.id : `${layer.name} ${layer.id}`).join(" → ")
}

export function cgroupDeviceKey(path: string | null, device: string): string {
  return JSON.stringify([path, device])
}

export function exactDeviceId(row: DataRow): string | null {
  const major = rawText(value(row, "major"))
  const minor = rawText(value(row, "minor"))
  return major === null || minor === null ? null : `${major}:${minor}`
}

function association(mount: DataRow, via: string | null): CgroupMountAssociation | null {
  const mountPoint = rawText(value(mount, "mount_point"))
  if (mountPoint === null) return null
  return {
    infrastructure: storedBoolean(value(mount, "is_k8s_infra")),
    mountPoint,
    root: rawText(value(mount, "root")),
    source: rawText(value(mount, "source")),
    via,
  }
}

// One exact diskstats name per device; two names for one identity name nothing.
function deviceNames(devices: readonly DataRow[]): ReadonlyMap<string, string | null> {
  const names = new Map<string, string | null>()
  for (const device of devices) {
    const id = exactDeviceId(device)
    const name = rawText(value(device, "device"))
    if (id === null || name === null) continue
    const stored = names.get(id)
    names.set(id, stored === undefined || stored === name ? name : null)
  }
  return names
}

function childIndex(parents: BlockParents): ReadonlyMap<string, readonly string[]> {
  const children = new Map<string, string[]>()
  for (const [child, below] of parents) {
    for (const parent of below) {
      const stored = children.get(parent) ?? []
      stored.push(child)
      children.set(parent, stored)
    }
  }
  for (const stored of children.values()) stored.sort(compareText)
  return children
}

// Breadth-first closure over exact edges, nearest layers first.
function reachable(start: string, next: (device: string) => readonly string[]): readonly string[] {
  const seen = new Set<string>([start])
  const queue = [start]
  const found: string[] = []
  for (let index = 0; index < queue.length; index += 1) {
    for (const device of next(queue[index]!)) {
      if (seen.has(device)) continue
      seen.add(device)
      found.push(device)
      queue.push(device)
    }
  }
  return found
}

// The shortest exact path from one layer down to another, both included.
function route(from: string, to: string, next: (device: string) => readonly string[]): readonly string[] {
  if (from === to) return [from]
  const previous = new Map<string, string>([[from, from]])
  const queue = [from]
  for (let index = 0; index < queue.length; index += 1) {
    for (const device of next(queue[index]!)) {
      if (previous.has(device)) continue
      previous.set(device, queue[index]!)
      if (device !== to) {
        queue.push(device)
        continue
      }
      const path = [to]
      let step = to
      while (step !== from) {
        step = previous.get(step) ?? from
        path.unshift(step)
      }
      return path
    }
  }
  return [from, to]
}

// Down the first exact edge at every layer until a device has none beneath it.
function descend(start: string, parents: BlockParents): readonly string[] {
  const path = [start]
  let next = parents.get(start)?.[0]
  while (next !== undefined && !path.includes(next)) {
    path.push(next)
    next = parents.get(next)?.[0]
  }
  return path
}

function sourceBasename(source: string | null): string | null {
  if (source === null) return null
  return source.split("/").filter((part) => part !== "").at(-1) ?? source
}

function associationKey(association: CgroupMountAssociation): string {
  return [association.mountPoint, association.root ?? "", association.source ?? "", String(association.infrastructure), association.via ?? ""].join(" ")
}

function compareAssociations(left: CgroupMountAssociation, right: CgroupMountAssociation): number {
  const leftRank = left.infrastructure === true ? 1 : 0
  const rightRank = right.infrastructure === true ? 1 : 0
  return leftRank - rightRank
    || (left.via === null ? 0 : 1) - (right.via === null ? 0 : 1)
    || left.mountPoint.length - right.mountPoint.length
    || compareText(left.mountPoint, right.mountPoint)
    || compareText(left.root ?? "", right.root ?? "")
    || compareText(left.source ?? "", right.source ?? "")
}

function compareText(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0
}

function storedBoolean(cell: Cell): boolean | null {
  if (typeof cell === "boolean") return cell
  if (typeof cell === "number") return cell !== 0
  if (typeof cell === "string" && ["true", "1"].includes(cell.toLowerCase())) return true
  if (typeof cell === "string" && ["false", "0"].includes(cell.toLowerCase())) return false
  return null
}
