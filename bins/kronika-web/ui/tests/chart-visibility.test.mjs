import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

test("the compact timeline is permanent and the full chart opens in Inspector", async () => {
  const app = await readFile(new URL("../src/app.tsx", import.meta.url), "utf8")
  const timeline = await readFile(new URL("../src/timeline.tsx", import.meta.url), "utf8")
  const styles = await readFile(new URL("../src/styles.css", import.meta.url), "utf8")
  assert.match(app, /data-testid="charts-toggle"/)
  assert.match(app, /presentation="inspector"/)
  assert.doesNotMatch(app, /kronika\.charts|chartsVisible|ChartVisibilityProvider/)
  assert.match(timeline, /presentation = "preview"/)
  assert.match(styles, /\.timeline-preview \{[^}]*height: 124px;/s)
})
