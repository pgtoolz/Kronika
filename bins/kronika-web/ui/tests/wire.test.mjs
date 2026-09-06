import assert from "node:assert/strict"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const { readNdjson } = await importModule('export { readNdjson } from "../src/wire.ts"')

const stream = (chunks) => new Response(new ReadableStream({
  start(controller) {
    for (const chunk of chunks) controller.enqueue(new TextEncoder().encode(chunk))
    controller.close()
  },
}))

test("the byte counter reports exactly the bytes that arrived", async () => {
  const calls = []
  const records = await readNdjson(stream(['{"record":"a"}\n{"record":"b', '"}\n']), "test", new AbortController().signal, (received) => calls.push(received))
  assert.equal(records.length, 2)
  const total = new TextEncoder().encode('{"record":"a"}\n{"record":"b"}\n').byteLength
  assert.ok(calls.length >= 1, JSON.stringify(calls))
  assert.equal(calls.at(-1), total)
  assert.ok(calls.every((received, index) => received > (calls[index - 1] ?? 0) && received <= total), JSON.stringify(calls))
})

test("a bodyless response counts its full text once", async () => {
  const calls = []
  const response = new Response('{"record":"a"}\n')
  Object.defineProperty(response, "body", { value: null })
  const records = await readNdjson(response, "test", new AbortController().signal, (received) => calls.push(received))
  assert.equal(records.length, 1)
  assert.deepEqual(calls, ['{"record":"a"}\n'.length])
})

test("no counter, no change: the stream parses as before", async () => {
  const records = await readNdjson(stream(['{"record":"a"}\n', '{"record":"b"}\n']), "test", new AbortController().signal)
  assert.deepEqual(records.map((record) => record.record), ["a", "b"])
})
