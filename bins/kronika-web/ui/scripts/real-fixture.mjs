import { createHash } from "node:crypto"
import { mkdir, readFile, writeFile } from "node:fs/promises"
import { basename, dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { gunzipSync } from "node:zlib"

import { gzipSync } from "fflate"

const scriptsDirectory = dirname(fileURLToPath(import.meta.url))
const uiDirectory = resolve(scriptsDirectory, "..")
const fixturePath = join(uiDirectory, "fixtures/real-hour.json.gz")
const manifestPath = join(uiDirectory, "fixtures/real-hour.manifest.json")
const sourceHtmlSha256 = "064925389fca0e3edda0b73e88e6be158b0e4707f51b9b0935da6a0c72b9ec60"
const expected = {
  findings: 2_884,
  osRows: 111_673,
  osSnapshots: 420,
  pgRows: 2_888,
  pgSnapshots: 70,
  pidJoins: 2_755,
}

const recoverAt = process.argv.indexOf("--recover")
if (recoverAt >= 0) {
  const source = process.argv[recoverAt + 1]
  if (source === undefined || process.argv.length !== recoverAt + 2) {
    throw new Error("usage: node scripts/real-fixture.mjs --recover SOURCE_HTML")
  }
  await recover(resolve(source))
} else if (process.argv.length === 2 || process.argv.includes("--check")) {
  await check()
} else {
  throw new Error("usage: node scripts/real-fixture.mjs [--check | --recover SOURCE_HTML]")
}

async function recover(source) {
  const html = await readFile(source)
  if (sha256(html) !== sourceHtmlSha256) {
    throw new Error("the recovery HTML does not match the owner-approved source")
  }
  const raw = extractData(html.toString("utf8"))
  const data = parseAndValidate(raw)
  scanSecrets(data)
  const compressed = gzip(raw)
  const manifest = makeManifest(raw, compressed, data, basename(source))
  await mkdir(dirname(fixturePath), { recursive: true })
  await writeFile(fixturePath, compressed)
  await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`)
  process.stdout.write(summary(data, raw, compressed))
}

async function check() {
  const [compressed, encodedManifest] = await Promise.all([
    readFile(fixturePath),
    readFile(manifestPath, "utf8"),
  ])
  validateGzipHeader(compressed)
  const raw = gunzipSync(compressed)
  const data = parseAndValidate(raw)
  scanSecrets(data)
  const manifest = JSON.parse(encodedManifest)
  const rebuilt = gzip(raw)
  if (!rebuilt.equals(compressed)) {
    throw new Error("the real-hour fixture gzip is not deterministic")
  }
  const actual = makeManifest(raw, compressed, data, manifest.source)
  if (JSON.stringify(actual) !== JSON.stringify(manifest)) {
    throw new Error("the real-hour fixture manifest does not match the fixture")
  }
  process.stdout.write(summary(data, raw, compressed))
}

function extractData(html) {
  const marker = "const DATA="
  const start = html.indexOf(marker)
  if (start < 0) throw new Error("the recovery HTML has no DATA object")
  const from = start + marker.length
  let depth = 0
  let quoted = false
  let escaped = false
  for (let at = from; at < html.length; at += 1) {
    const character = html[at]
    if (quoted) {
      if (escaped) escaped = false
      else if (character === "\\") escaped = true
      else if (character === '"') quoted = false
      continue
    }
    if (character === '"') quoted = true
    else if (character === "{" || character === "[") depth += 1
    else if (character === "}" || character === "]") depth -= 1
    else if (character === ";" && depth === 0) return Buffer.from(html.slice(from, at))
  }
  throw new Error("the recovery HTML DATA object is incomplete")
}

function parseAndValidate(raw) {
  let data
  try {
    data = JSON.parse(raw)
  } catch {
    throw new Error("the real-hour fixture is not valid JSON")
  }
  const osRows = rows(data.os)
  const pgRows = rows(data.pg)
  const actual = {
    findings: length(data.findings, "findings"),
    osRows,
    osSnapshots: length(data.os?.snapshots, "os.snapshots"),
    pgRows,
    pgSnapshots: length(data.pg?.snapshots, "pg.snapshots"),
    pidJoins: number(data.meta?.pidJoins, "meta.pidJoins"),
  }
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error("the real-hour fixture cardinalities changed")
  }
  if (number(data.redactions?.total, "redactions.total") !== 0
      || length(data.redactions?.audit, "redactions.audit") !== 0) {
    throw new Error("the real-hour fixture has an unexpected redaction record")
  }
  validateTable(data.os, "os")
  validateTable(data.pg, "pg")
  for (const finding of data.findings) {
    if (finding === null || typeof finding !== "object"
        || !["known_bad", "spike", "event"].includes(finding.kind)
        || !integerText(finding.segment_id) || !integerText(finding.type_id)
        || !integerText(finding.t) || !Number.isInteger(finding.row_ordinal)
        || !Number.isInteger(finding.field_ordinal)) {
      throw new Error("the real-hour fixture has an invalid finding locator")
    }
  }
  return data
}

function validateTable(table, name) {
  if (table === null || typeof table !== "object" || !Array.isArray(table.columns)
      || table.columns.length === 0 || new Set(table.columns).size !== table.columns.length) {
    throw new Error(`${name} fixture columns are invalid`)
  }
  for (const snapshot of table.snapshots) {
    if (snapshot === null || typeof snapshot !== "object" || !Array.isArray(snapshot.rows)
        || !integerText(snapshot.segment_id) || !integerText(snapshot.type_id)
        || !integerText(snapshot.ts)) {
      throw new Error(`${name} fixture snapshot coordinates are invalid`)
    }
    if (snapshot.rows.some((row) => !Array.isArray(row) || row.length !== table.columns.length)) {
      throw new Error(`${name} fixture row width does not match its columns`)
    }
  }
}

function scanSecrets(data) {
  const signatures = [
    /-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----/i,
    /\bBasic\s+[A-Za-z0-9+/]{12,}={0,2}\b/i,
    /\bBearer\s+[A-Za-z0-9._~+/-]{12,}\b/i,
    /\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b/,
    /\b(?:AKIA|ASIA)[A-Z0-9]{16}\b/,
    /\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{60,})\b/,
    /\bxox[baprs]-[A-Za-z0-9-]{10,}\b/,
    /\b(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{16,}\b/,
    /\bAIza[0-9A-Za-z_-]{30,}\b/,
    /(?:postgres(?:ql)?|mysql|mariadb|mongodb(?:\+srv)?|redis):\/\/[^\s:/]+:[^\s@/]+@/i,
    /\bhttps?:\/\/[^\s:/]+:[^\s@/]+@/i,
    /(?:^|[\s;&])(?:PGPASSWORD|PASSWORD|PASSWD|TOKEN|API_KEY|SECRET|AUTHORIZATION|COOKIE|PRIVATE_KEY)\s*=\s*\S+/i,
    /(?:^|\s)--?(?:password|passwd|token|api[-_]?key|secret|authorization|cookie|private[-_]?key)(?:[=\s]+)\S+/i,
    /(?:create|alter)\s+(?:user|role)[\s\S]{0,300}\bpassword\b/i,
  ]
  const sensitiveKey = /(?:^|_)(?:password|passwd|token|api_?key|secret|authorization|cookie|private_?key|dsn)(?:$|_)/i
  let failed = false
  walk(data, (key, value) => {
    if (sensitiveKey.test(key)) failed = true
    if (typeof value === "string" && signatures.some((signature) => signature.test(value))) failed = true
  })
  if (failed) throw new Error("the real-hour fixture contains a credential or secret signature")
}

function walk(value, visit, key = "") {
  visit(key, value)
  if (Array.isArray(value)) {
    for (const item of value) walk(item, visit)
  } else if (value !== null && typeof value === "object") {
    for (const [childKey, child] of Object.entries(value)) walk(child, visit, childKey)
  }
}

function makeManifest(raw, compressed, data, source) {
  return {
    format: "kronika-ui-real-hour-v1",
    source,
    source_html_sha256: sourceHtmlSha256,
    fixture_json_sha256: sha256(raw),
    fixture_gzip_sha256: sha256(compressed),
    fixture_json_bytes: raw.length,
    fixture_gzip_bytes: compressed.length,
    capture_from_us: data.meta.captureFromUs,
    capture_to_us: data.meta.captureToUs,
    os_snapshots: data.os.snapshots.length,
    os_rows: rows(data.os),
    postgresql_activity_snapshots: data.pg.snapshots.length,
    postgresql_activity_rows: rows(data.pg),
    findings: data.findings.length,
    exact_pid_joins: data.meta.pidJoins,
  }
}

function rows(table) {
  return length(table?.snapshots, "snapshots")
    && table.snapshots.reduce((total, snapshot) => total + length(snapshot.rows, "snapshot.rows"), 0)
}

function length(value, name) {
  if (!Array.isArray(value)) throw new Error(`${name} is not an array`)
  return value.length
}

function number(value, name) {
  if (!Number.isSafeInteger(value) || value < 0) throw new Error(`${name} is not a nonnegative integer`)
  return value
}

function integerText(value) {
  return typeof value === "string" && /^\d+$/.test(value)
}

function gzip(raw) {
  const compressed = Buffer.from(gzipSync(raw, { level: 9, mtime: 0 }))
  validateGzipHeader(compressed)
  return compressed
}

function validateGzipHeader(bytes) {
  if (bytes[0] !== 0x1f || bytes[1] !== 0x8b || bytes[2] !== 8 || bytes[3] !== 0
      || !bytes.subarray(4, 8).equals(Buffer.alloc(4))) {
    throw new Error("the real-hour fixture must use deterministic unnamed gzip")
  }
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex")
}

function summary(data, raw, compressed) {
  return `real-hour fixture os_snapshots=${data.os.snapshots.length} os_rows=${rows(data.os)} pg_snapshots=${data.pg.snapshots.length} pg_rows=${rows(data.pg)} findings=${data.findings.length} json=${raw.length} gzip=${compressed.length}\n`
}
