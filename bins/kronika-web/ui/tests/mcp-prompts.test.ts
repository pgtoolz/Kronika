import assert from "node:assert/strict"
import test from "node:test"

import {
  type Auth,
  claudeCommand,
  claudePrompt,
  codexPrompt,
  codexTable,
  credentialsKey,
  cursorConfig,
  cursorPrompt,
  serverName,
} from "../src/mcp-prompts.ts"

const ENDPOINT = "http://192.168.0.22:8080/mcp"
const NAME = "kronika-192-168-0-22-8080"
const NAMED = "kronika-billing-192-168-0-22-8080"
const HEADER: Auth = { kind: "header", value: "Basic ZGVtbzpmb3JlbnNpY3M=" }
const OPEN: Auth = { kind: "open" }
const PLACEHOLDER: Auth = { kind: "placeholder" }
const t = (key: string) => key

test("the cursor config parses as JSON in every mode and carries the header only when one exists", () => {
  for (const auth of [HEADER, OPEN, PLACEHOLDER]) {
    const parsed = JSON.parse(cursorConfig(ENDPOINT, auth, null)) as {
      mcpServers: Record<string, { url: string; headers?: { Authorization: string } }>
    }
    const server = parsed.mcpServers[NAME]
    assert.equal(server?.url, ENDPOINT)
    if (auth.kind === "open") assert.equal(server?.headers, undefined)
    else if (auth.kind === "header") {
      assert.equal(server?.headers?.Authorization, HEADER.value)
    } else assert.equal(server?.headers?.Authorization, "Basic <BASE64>")
  }
})

test("the codex table omits http_headers on an open server and inlines the served value", () => {
  assert.deepEqual(codexTable(ENDPOINT, OPEN, null), [`[mcp_servers.${NAME}]`, `url = "${ENDPOINT}"`])
  assert.equal(
    codexTable(ENDPOINT, HEADER, null).at(-1),
    `http_headers = { "Authorization" = "${HEADER.value}" }`,
  )
  assert.equal(
    codexTable(ENDPOINT, PLACEHOLDER, null).at(-1),
    'http_headers = { "Authorization" = "Basic <BASE64>" }',
  )
})

test("the claude command drops the continuation backslash together with the header line", () => {
  assert.deepEqual(claudeCommand(ENDPOINT, OPEN, null), [
    `claude mcp add --transport http --scope user ${NAME} ${ENDPOINT}`,
  ])
  const [first, second] = claudeCommand(ENDPOINT, HEADER, null)
  assert.ok(first?.endsWith("\\"))
  assert.equal(second, `  --header "Authorization: ${HEADER.value}"`)
  // The placeholder form computes base64 in place, wrap-safe for long
  // credentials.
  assert.match(claudeCommand(ENDPOINT, PLACEHOLDER, null)[1] ?? "", /base64 \| tr -d '\\n'/)
})

test("the base64 recipe appears only where a <BASE64> placeholder is left to fill", () => {
  for (const [prompt, auth, expected] of [
    [codexPrompt, PLACEHOLDER, true],
    [codexPrompt, HEADER, false],
    [codexPrompt, OPEN, false],
    [cursorPrompt, PLACEHOLDER, true],
    [cursorPrompt, HEADER, false],
    [claudePrompt, HEADER, false],
    [claudePrompt, PLACEHOLDER, false],
  ] as const) {
    assert.equal(prompt(ENDPOINT, auth, t, null).includes("mcp.prompt.base64"), expected)
  }
})

test("the server name is one registration per instance", () => {
  assert.equal(serverName(ENDPOINT, null), NAME)
  assert.equal(serverName(ENDPOINT, "billing"), NAMED)
  assert.equal(serverName(ENDPOINT, "Прод База"), NAME)
  assert.equal(serverName(ENDPOINT, "My_DB.v2"), "kronika-my-db-v2-192-168-0-22-8080")
  assert.equal(serverName("not a url", "billing"), "kronika-billing")
  assert.equal(serverName("http://demo.local/mcp", null), "kronika-demo-local")
  assert.equal(serverName("https://Db-Host.Example.com:8443/mcp", null), "kronika-db-host-example-com-8443")
  assert.equal(serverName("http://[::1]:8088/mcp", null), "kronika-1-8088")
  assert.equal(serverName("not a url", null), "kronika")
  assert.notEqual(serverName("http://192.168.0.20:8088/mcp", null), serverName("http://192.168.0.20:18090/mcp", null))
})

test("every prompt threads the derived name through each slotted key", () => {
  const slotted = (key: string, slots?: Record<string, unknown>) =>
    slots === undefined ? key : `${key} ${JSON.stringify(slots)}`
  for (const [prompt, slots] of [
    [claudePrompt, 1],
    [codexPrompt, 2],
    [cursorPrompt, 1],
  ] as const) {
    const text = prompt(ENDPOINT, OPEN, slotted, null)
    assert.equal(text.split(`"name":"${NAME}"`).length - 1, slots, text)
    const named = prompt(ENDPOINT, OPEN, slotted, "billing")
    assert.equal(named.split(`"name":"${NAMED}"`).length - 1, slots, named)
  }
})

test("the credentials note names what the prompts carry", () => {
  assert.equal(credentialsKey(HEADER), "mcp.credentials.embedded")
  assert.equal(credentialsKey(OPEN), "mcp.credentials.open")
  assert.equal(credentialsKey(PLACEHOLDER), "mcp.credentials.manual")
})
