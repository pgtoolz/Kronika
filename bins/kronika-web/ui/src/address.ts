export interface Address {
  readonly at: number | null
  readonly view: View
  readonly lens: Lens
  readonly pgLens: PgLens
  readonly pgLevel: PgLevel
  readonly datid: string | null
  readonly schema: string | null
  readonly relid: string | null
  readonly indexrelid: string | null
  readonly tablespaceOid: string | null
  readonly sort: { readonly column: string; readonly descending: boolean } | null
  readonly row: string | null
  readonly panel: InspectorPanel
  readonly find: string
  readonly metric: string | null
}

export const INSPECTOR_PANELS = ["chart", "detail", "pg_stat_activity", "os_process", "pg_stat_statements", "pg_store_plans"] as const

export type InspectorPanel = (typeof INSPECTOR_PANELS)[number] | null

// Detail is implied by a row and omitted from the URL.
function addressablePanel(panel: string | null): InspectorPanel {
  const known = INSPECTOR_PANELS.find((candidate) => candidate === panel) ?? null
  return known === "detail" ? null : known
}

export type View =
  | "host"
  | "processes"
  | "pg.overview"
  | "pg.activity"
  | "pg.vacuum"
  | "pg.statements"
  | "pg.plans"
  | "pg.locks"
  | "pg.databases"
  | "pg.tables"
  | "pg.indexes"
  | "events"

type Lens = "generic" | "cpu" | "memory" | "disk" | "tree"
export type Source = "host" | "processes" | "postgresql" | "events"
export type PgLens = "load" | "per_call" | "io" | "resources" | "stability" | "timing" | "identity"
  | "access" | "changes" | "maintenance" | "size_buffers" | "freeze"
  | "usage" | "low_activity" | "state"
export type PgLevel = "database" | "schema" | "tablespace" | "object"

const VIEWS: readonly View[] = [
  "host",
  "processes",
  "pg.overview", "pg.activity", "pg.vacuum", "pg.statements", "pg.plans", "pg.locks", "pg.databases", "pg.tables", "pg.indexes",
  "events",
]

const LENSES: readonly Lens[] = ["generic", "cpu", "memory", "disk", "tree"]
const PG_LENSES: readonly PgLens[] = ["load", "per_call", "io", "resources", "stability", "timing", "identity", "access", "changes", "maintenance", "size_buffers", "freeze", "usage", "low_activity", "state"]
const PG_LEVELS: readonly PgLevel[] = ["database", "schema", "tablespace", "object"]

export const DEFAULT_ADDRESS: Address = {
  at: null,
  view: "processes",
  lens: "tree",
  pgLens: "load",
  pgLevel: "object",
  datid: null,
  schema: null,
  relid: null,
  indexrelid: null,
  tablespaceOid: null,
  sort: null,
  row: null,
  panel: null,
  find: "",
  metric: null,
}

export function readAddress(search: string): Address {
  const parameters = new URLSearchParams(search.startsWith("?") ? search.slice(1) : search)
  const at = Number.parseInt(parameters.get("at") ?? "", 10)
  const view = parameters.get("view")
  const lens = parameters.get("lens")
  const pgLens = parameters.get("pg_lens")
  const pgLevel = PG_LEVELS.find((known) => known === parameters.get("level")) ?? DEFAULT_ADDRESS.pgLevel
  // Legacy host.* views map to Host.
  const resolvedView = VIEWS.find((known) => known === view)
    ?? (view !== null && view.startsWith("host.") ? "host" as View : DEFAULT_ADDRESS.view)
  const relation = resolvedView === "pg.tables" || resolvedView === "pg.indexes"
  const postgresEntity = isPostgresEntityView(resolvedView)
  const host = resolvedView === "host"
  const datid = relation && (pgLevel === "schema" || pgLevel === "object") ? oid(parameters.get("datid")) : null
  const sort = parameters.get("sort") ?? ""
  const column = sort.startsWith("-") ? sort.slice(1) : sort
  const row = postgresEntity
    ? postgresEntityRow(parameters.get("row"))
    : resolvedView === "processes" || relation || host || resolvedView === "events" ? parameters.get("row") : null
  const metric = host && /^[a-z0-9_.-]+$/.test(parameters.get("metric") ?? "") ? parameters.get("metric") : null
  const requestedPanel = parameters.get("panel")
  return {
    at: Number.isSafeInteger(at) && at > 0 ? at : null,
    view: resolvedView,
    lens: LENSES.find((known) => known === lens) ?? DEFAULT_ADDRESS.lens,
    pgLens: PG_LENSES.find((known) => known === pgLens) ?? DEFAULT_ADDRESS.pgLens,
    pgLevel: relation ? pgLevel : DEFAULT_ADDRESS.pgLevel,
    datid,
    schema: relation && pgLevel === "object" && datid !== null ? parameters.get("schema") : null,
    relid: relation && pgLevel === "object" && datid !== null ? oid(parameters.get("relid")) : null,
    indexrelid: resolvedView === "pg.indexes" && pgLevel === "object" && datid !== null ? oid(parameters.get("indexrelid")) : null,
    tablespaceOid: relation && pgLevel === "object" ? oid(parameters.get("tablespace_oid")) : null,
    sort: column === "" ? null : { column, descending: sort.startsWith("-") },
    row,
    // Legacy row-only links open Detail; Chart does not require a row.
    panel: addressablePanel(requestedPanel) ?? (row !== null && row !== "" ? "detail" : null),
    find: parameters.get("find") ?? "",
    metric,
  }
}

export function writeAddress(address: Address): string {
  const parameters = new URLSearchParams()
  const relation = address.view === "pg.tables" || address.view === "pg.indexes"
  if (address.at !== null) parameters.set("at", String(address.at))
  if (address.view !== DEFAULT_ADDRESS.view) parameters.set("view", address.view)
  if (address.lens !== DEFAULT_ADDRESS.lens && address.view === "processes") parameters.set("lens", address.lens)
  if (address.pgLens !== DEFAULT_ADDRESS.pgLens && (relation || address.view === "pg.statements" || address.view === "pg.plans")) parameters.set("pg_lens", address.pgLens)
  if (relation && address.pgLevel !== DEFAULT_ADDRESS.pgLevel) parameters.set("level", address.pgLevel)
  if (relation && address.pgLevel !== "database" && address.datid !== null) parameters.set("datid", address.datid)
  if (relation && address.pgLevel === "object" && address.schema !== null) parameters.set("schema", address.schema)
  if (relation && address.pgLevel === "object" && address.relid !== null) parameters.set("relid", address.relid)
  if (address.view === "pg.indexes" && address.pgLevel === "object" && address.indexrelid !== null) parameters.set("indexrelid", address.indexrelid)
  if (relation && address.pgLevel === "object" && address.tablespaceOid !== null) parameters.set("tablespace_oid", address.tablespaceOid)
  if (address.sort !== null) parameters.set("sort", `${address.sort.descending ? "-" : ""}${address.sort.column}`)
  if ((address.view === "processes" || relation || address.view === "host" || address.view === "events"
      || (isPostgresEntityView(address.view) && postgresEntityRow(address.row) !== null))
    && address.row !== null && address.row !== "") parameters.set("row", address.row)
  if (address.panel !== null && address.panel !== "detail") parameters.set("panel", address.panel)
  if (address.find !== "") parameters.set("find", address.find)
  if (address.view === "host" && address.metric !== null) parameters.set("metric", address.metric)
  const query = parameters.toString()
  return query === "" ? "/" : `/?${query}`
}

function oid(stored: string | null): string | null {
  return stored !== null && /^[1-9]\d*$/.test(stored) && Number(stored) <= 4_294_967_295 ? stored : null
}

function isPostgresEntityView(view: View): boolean {
  return view === "pg.activity" || view === "pg.vacuum" || view === "pg.statements" || view === "pg.plans"
    || view === "pg.locks" || view === "pg.databases"
}

function postgresEntityRow(stored: string | null): string | null {
  return stored !== null && /^[^:]+:\d+:[^:]+$/.test(stored) ? stored : null
}

export function viewOf(source: string, pgSection: string): View {
  if (source === "events") return "events"
  if (source === "processes") return "processes"
  if (source === "postgresql") return `pg.${pgSection}` as View
  return "host"
}

export function sourceOf(view: View): Source {
  if (view === "events" || view === "processes") return view
  return view.startsWith("pg.") ? "postgresql" : "host"
}

export function pgSectionOf(view: View): "overview" | "activity" | "vacuum" | "statements" | "plans" | "locks" | "databases" | "tables" | "indexes" {
  const section = view.startsWith("pg.") ? view.slice(3) : "overview"
  return section as "overview" | "activity" | "vacuum" | "statements" | "plans" | "locks" | "databases" | "tables" | "indexes"
}

export function stepOf(address: string): string {
  return address.replace(/(\?|&)at=\d+&?/, "$1").replace(/[?&]$/, "")
}
