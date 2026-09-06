import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

test("the MCP panel is reachable from the top bar and self-addresses the page origin", async () => {
  const [app, panel] = await Promise.all([
    readFile(new URL("../src/app.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/mcp-connect.tsx", import.meta.url), "utf8"),
  ])
  assert.match(app, /data-testid="mcp-trigger"/)
  // Opening one full-height surface closes the other.
  assert.match(app, /setMcpOpen\(\(current\) => !current\);?\s*setHelpOpen\(false\)/)
  assert.match(app, /setHelpOpen\(\(current\) => !current\);?\s*setMcpOpen\(false\)/)
  // The app-level keyboard fallback closes both panels when neither modal
  // owns the event.
  assert.match(app, /event\.key === "\?"(?:(?!event\.key)[\s\S])*?setMcpOpen\(false\)/)
  assert.match(app, /event\.key === "Escape"(?:(?!event\.key)[\s\S])*?setMcpOpen\(false\)/)
  assert.match(app, /\{!KRONIKA_REPORT && mcpOpen && <McpPanel/)
  assert.match(panel, /window\.location\.origin\}\/mcp/)
  for (const label of ["Claude Code", "Codex CLI", "Cursor"]) assert.match(panel, new RegExp(label))
  // One client at a time behind a segmented tab row, in the app's own
  // lens-tabs shape.
  assert.match(panel, /className="lens-tabs[^"]*" role="group"/)
  assert.match(panel, /aria-pressed=\{client === candidate\.label\}/)
  assert.match(panel, /data-testid=\{`mcp-tab-\$\{candidate\.id\}`\}/)
  // The row must actually switch: a click updates the state and only the
  // selected client's builder runs.
  assert.match(panel, /onClick=\{\(\) => \{ setClient\(candidate\.label\); setCopied\(false\) \}\}/)
  // Copying must go through the shared helper: plain-http origins have no
  // navigator.clipboard, and the raw optional chain swallowed the click.
  assert.match(panel, /void copyText\(prompt, t\("clipboard\.manual"\)\)\.then\(setCopied\)/)
  assert.match(panel, /selected\.builder\(url, auth, t, database\)/)
  // The Authorization value comes from the server, through the session
  // fetch wrapper that keeps UI 401s challenge-free.
  assert.match(panel, /apiFetch\("\/api\/mcp-access"/)
  assert.doesNotMatch(panel, /\bfetch\(/)
  assert.match(panel, /kind: "header", value: body\.authorization/)
  assert.match(panel, /catch\(\(\) => setAuth\(\{ kind: "placeholder" \}\)\)/)
})

test("the MCP drawer is a native modal that owns focus and Escape", async () => {
  const panel = await readFile(new URL("../src/mcp-connect.tsx", import.meta.url), "utf8")
  assert.match(panel, /<dialog aria-labelledby=\{titleId\} aria-modal="true"/)
  assert.match(panel, /<h2 id=\{titleId\}>/)
  assert.match(panel, /role="dialog"/)
  assert.match(panel, /dialog\.current\?\.showModal\(\)/)
  assert.match(panel, /closeButton\.current\?\.focus\(\{ preventScroll: true \}\)/)
  assert.match(panel, /onCancel=\{\(event\) => \{ event\.preventDefault\(\); dismiss\(\) \}\}/)
  assert.match(panel, /onKeyDown=\{\(event\) => event\.stopPropagation\(\)\}/)
  assert.match(panel, /if \(target\?\.isConnected\) target\.focus\(\{ preventScroll: true \}\)/)
  assert.doesNotMatch(panel, /querySelectorAll/)
  assert.doesNotMatch(panel, /<aside/)
})

test("every client prompt carries the endpoint and wrap-safe base64", async () => {
  const panel = await readFile(new URL("../src/mcp-prompts.ts", import.meta.url), "utf8")
  // tr -d keeps long credentials from folding a newline into the header:
  // once in the placeholder claude command, once in the shared recipe.
  assert.equal(panel.match(/base64 \| tr -d '\\\\n'/g)?.length, 2)
  assert.match(panel, /--transport http --scope user \$\{name\} \$\{url\}/)
  const app = await readFile(new URL("../src/app.tsx", import.meta.url), "utf8")
  assert.match(app, /apiFetch\("\/api\/instance-label"/)
  assert.match(app, /document\.title = database === null \? "Kronika" : `\$\{database\} — Kronika`/)
  assert.match(app, /<McpPanel database=\{database\}/)
  const connect = await readFile(new URL("../src/mcp-connect.tsx", import.meta.url), "utf8")
  assert.match(connect, /selected\.builder\(url, auth, t, database\)/)
  const english = await readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8")
  assert.match(english, /replace the whole \[mcp_servers\.\{name\}\] table/)
  assert.match(english, /mcp\.docs: .*docs\/mcp-clients\.md/)
  const russian = await readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8")
  assert.match(russian, /замени целиком таблицу \[mcp_servers\.\{name\}\]/)
  assert.match(russian, /mcp\.docs: .*docs\/mcp-clients\.ru\.md/)
})

test("no component calls navigator.clipboard directly", async () => {
  const { readdir } = await import("node:fs/promises")
  const sources = (await readdir(new URL("../src", import.meta.url), { recursive: true })).filter(
    (name) => (name.endsWith(".ts") || name.endsWith(".tsx")) && name !== "clipboard.ts",
  )
  for (const name of sources) {
    const source = await readFile(new URL(`../src/${name}`, import.meta.url), "utf8")
    assert.doesNotMatch(source, /navigator\.clipboard/, name)
  }
})
