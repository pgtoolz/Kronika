import assert from "node:assert/strict"
import test from "node:test"

import { globMatcher } from "../src/glob.ts"

test("a pattern without wildcards matches a substring", () => {
  const match = globMatcher("postgres")
  assert.ok(match !== null)
  assert.equal(match("postgres: checkpointer"), true)
  assert.equal(match("POSTGRES: walwriter"), true)
  assert.equal(match("kthreadd"), false)
})

test("a wildcard pattern is looked for inside the value, not against all of it", () => {
  const star = globMatcher("kroni*")
  assert.ok(star !== null)
  assert.equal(star("/pgdata/.kronika-pr12-live/bin/kronika-collector"), true)
  assert.equal(star("kronika-web"), true)
  assert.equal(star("/usr/bin/postgres"), false)

  const single = globMatcher("cpuhp/?")
  assert.ok(single !== null)
  assert.equal(single("cpuhp/2"), true)
  assert.equal(single("/sbin/cpuhp/7 --daemon"), true)
  assert.equal(single("cpuhp/"), false)
})

test("a wildcard matches intervening characters", () => {
  const match = globMatcher("postgres*checkpointer")
  assert.ok(match !== null)
  assert.equal(match("postgres: sgerp checkpointer"), true)
  assert.equal(match("postgres: walwriter"), false)
})

test("regular expression characters in a pattern are literal", () => {
  const match = globMatcher("kworker/*:0H-*")
  assert.ok(match !== null)
  assert.equal(match("kworker/0:0H-events_highpri"), true)

  const dotted = globMatcher("10.100.0.4")
  assert.ok(dotted !== null)
  assert.equal(dotted("client 10.100.0.4:49518"), true)
  assert.equal(dotted("client 10a100b0c4"), false)
})

test("an empty pattern filters nothing", () => {
  assert.equal(globMatcher(""), null)
  assert.equal(globMatcher("   "), null)
})
