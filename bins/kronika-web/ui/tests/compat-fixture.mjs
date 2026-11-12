// Small wire-format fixture for cross-engine interaction tests, not backend acceptance.
export const HOUR = Date.UTC(2026, 7, 13, 5) * 1000
export const AT = HOUR + 1_800_000_000
const HOUR_US = 3_600_000_000
const SEGMENT = "7300"
const section = { logical_name: "pg_stat_activity", physical_name: "pg_stat_activity", type_id: "1001004", implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "256" }

export function answerFixture(request, response, html, exportReply) {
  const url = new URL(request.url, "http://127.0.0.1")
  if (url.pathname === "/") {
    response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
    response.end(html)
    return
  }
  if (url.pathname === "/auth/session") {
    response.writeHead(204)
    response.end()
    return
  }
  if (url.pathname === "/api/export") return exportReply(response)
  if (url.pathname === "/api/instance-label" || url.pathname === "/api/mcp-access") {
    response.setHeader("Content-Type", "application/json")
    response.end(JSON.stringify(url.pathname.endsWith("mcp-access") ? { authorization: null } : { database: "compat_fixture" }))
    return
  }
  let records = []
  if (url.pathname === "/api/hour") {
    const hour = Number(url.searchParams.get("from") ?? HOUR)
    records = [
      { record: "hour", from: String(hour), to: String(hour + HOUR_US - 1), available_hours: [HOUR - 24 * HOUR_US, HOUR, HOUR + HOUR_US].map(String) },
      { record: "lane_context", segment_id: SEGMENT, postgresql_interval_seconds: null, environment: 0 },
      { record: "catalog", from: String(hour), to: String(hour + HOUR_US - 1), source_families: [{ name: "postgresql", configured: true, present: true, metrics_present: true }] },
      { record: "finished_segment", id: SEGMENT, min_ts: String(hour), max_ts: String(hour + HOUR_US - 1), sections: [section] },
      ...[0, 10_000_000, 20_000_000].map((offset) => ({ record: "point", type_id: "0", series: "os_health", ts: String(hour + offset), identity: {}, value: 70 + offset / 1_000_000 })),
    ]
  } else if (url.pathname.endsWith("/snapshot")) {
    const columns = ["ts", "pid", "leader_pid", "datid", "datname", "usename", "application_name", "client_addr", "backend_type", "state", "wait_event_type", "wait_event", "query", "query_id"]
    records = [
      { record: "layout", rates: [], layout: { type_id: section.type_id, logical_name: section.logical_name, columns: columns.map((name) => ({ name })) } },
      { record: "row", segment_id: SEGMENT, type_id: section.type_id, ordinal: "1", timestamp: String(AT), values: [String(AT), 4242, null, 20, "compat_fixture", "kronika", "compat-test", "127.0.0.1", "client backend", "active", null, null, "select safari_compat", "991"] },
    ]
  } else if (url.pathname === "/api/snapshot/neighbor") {
    records = [{ record: "snapshot_neighbor", at: null, segment_id: null }]
  } else if (!["/api/catalog", "/api/events", "/api/heatmap"].includes(url.pathname)) {
    response.writeHead(404)
    response.end(`Unexpected fixture request: ${url.pathname}`)
    return
  }
  response.writeHead(200, { "Content-Type": "application/x-ndjson", "Cache-Control": "no-store" })
  response.end(records.map((record) => JSON.stringify(record)).join("\n") + "\n")
}
