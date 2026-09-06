import assert from "node:assert/strict"
import test from "node:test"

import { historyAddress } from "../src/build-mode.ts"

test("native navigation retains its root-relative address", () => {
  assert.equal(historyAddress("/?view=host", "/kronika/report.html", false), "/?view=host")
})

test("report navigation retains the HTML file pathname", () => {
  assert.equal(historyAddress("/", "/tmp/report.html", true), "/tmp/report.html")
  assert.equal(
    historyAddress("/?view=host&metric=cpu_used_cores", "/tmp/report.html", true),
    "/tmp/report.html?view=host&metric=cpu_used_cores",
  )
})
