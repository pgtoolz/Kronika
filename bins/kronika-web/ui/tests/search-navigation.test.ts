import assert from "node:assert/strict"
import test from "node:test"

import { importFile } from "./import-module.mjs"

const { findAfterSurfaceNavigation, searchSurfaceForLocation, searchSurfaceForSection } = await importFile("../src/search-navigation.ts")

test("ordinary cross-surface navigation drops a Process expression and Back can restore its URL value", () => {
  const processSurface = searchSurfaceForLocation("processes", "activity")
  const activitySurface = searchSurfaceForLocation("postgresql", "activity")
  const sourceFind = "cpu_cores>1"

  assert.equal(processSurface, "os_process")
  assert.equal(activitySurface, "pg_stat_activity")
  assert.equal(findAfterSurfaceNavigation(processSurface, activitySurface, sourceFind), "")
  assert.equal(sourceFind, "cpu_cores>1", "the source URL value remains available to browser history")
})

test("overlapping public field names do not make two surfaces equivalent", () => {
  assert.equal(findAfterSurfaceNavigation("pg_stat_statements", "pg_store_plans", "query_id:42"), "")
  assert.equal(findAfterSurfaceNavigation("pg_stat_user_tables", "pg_stat_user_indexes", "schema:public"), "")
})

test("same-surface navigation preserves find through lens, sort, hour, cursor, and relation level changes", () => {
  for (const surface of ["os_process", "pg_stat_statements", "pg_stat_user_tables"] as const) {
    assert.equal(findAfterSurfaceNavigation(surface, surface, "text:worker"), "text:worker")
  }
})

test("related navigation replaces the source expression with a canonical target expression", () => {
  assert.equal(
    findAfterSurfaceNavigation("pg_stat_statements", "pg_store_plans", "exec_time_rate>1s/s", "query_id:42"),
    "query_id:42",
  )
  assert.equal(searchSurfaceForSection("plans"), "pg_store_plans")
  assert.equal(searchSurfaceForSection("overview"), null)
})
