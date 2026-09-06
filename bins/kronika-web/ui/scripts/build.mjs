import { execFileSync } from "node:child_process"
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

import { build } from "esbuild"
import { gzip } from "pako"

import { dictionaryModule } from "./i18n.mjs"

const scriptsDirectory = dirname(fileURLToPath(import.meta.url))
const uiDirectory = resolve(scriptsDirectory, "..")
const repository = resolve(uiDirectory, "../../..")
const fixtureOutputAt = process.argv.indexOf("--fixture-output")
const fixtureOutput = fixtureOutputAt < 0 ? null : process.argv[fixtureOutputAt + 1]
const reportMode = process.argv.includes("--report")
if (fixtureOutputAt >= 0 && (fixtureOutput === undefined || process.argv.length !== fixtureOutputAt + 2)) {
  throw new Error("usage: node scripts/build.mjs [--check] [--report] | --fixture-output OUTPUT_GZIP")
}
if (reportMode && fixtureOutput !== null) throw new Error("--report and --fixture-output cannot be combined")
const artifact = fixtureOutput !== null
  ? resolve(fixtureOutput)
  : reportMode
    ? join(repository, "bins/kronika-report/assets/kronika-report-shell.html.gz")
    : join(uiDirectory, "kronika-ui.html.gz")
const checkOnly = process.argv.includes("--check")
if (checkOnly && fixtureOutput !== null) throw new Error("--check and --fixture-output cannot be combined")
const maximumRawBytes = fixtureOutput === null ? 1_600_000 : 40_000_000
const maximumGzipBytes = fixtureOutput === null ? 560_000 : 8_000_000
/* The UI ships self-contained, so the two product typefaces are embedded as
   data: URIs (the CSP allows font-src data:). Subsets stay binary in
   ui/assets/; this keeps styles.css readable and the build reproducible. */
const LATIN_RANGE = "U+0000-00FF, U+2000-206F, U+2190-21BB, U+2212, U+FEFF"
const CYRILLIC_RANGE = "U+0301, U+0400-045F, U+0490-0491, U+04B0-04B1, U+2116"
const FONT_FILES = [
  ["IBM Plex Sans", 400, "PlexSans-Latin-400.woff2", LATIN_RANGE],
  ["IBM Plex Sans", 400, "PlexSans-Cyrillic-400.woff2", CYRILLIC_RANGE],
  ["IBM Plex Sans", 500, "PlexSans-Latin-500.woff2", LATIN_RANGE],
  ["IBM Plex Sans", 500, "PlexSans-Cyrillic-500.woff2", CYRILLIC_RANGE],
  ["IBM Plex Sans", 600, "PlexSans-Latin-600.woff2", LATIN_RANGE],
  ["IBM Plex Sans", 600, "PlexSans-Cyrillic-600.woff2", CYRILLIC_RANGE],
  ["JetBrains Mono", 400, "JetBrainsMono-Latin.woff2", LATIN_RANGE],
  ["JetBrains Mono", 400, "JetBrainsMono-Cyrillic.woff2", CYRILLIC_RANGE],
]
const rustToolchain = process.env.RUST_TOOLCHAIN ?? "1.96.0"
const rustHost = execFileSync("rustc", [`+${rustToolchain}`, "-vV"], { encoding: "utf8" })
  .match(/^host: (.+)$/m)?.[1]
if (rustHost === undefined) {
  throw new Error("rustc did not report its host target")
}

const temporary = await mkdtemp(join(tmpdir(), "kronika-ui-"))
try {
  const registry = execFileSync(
    "cargo",
    [`+${rustToolchain}`, "run", "--locked", "--quiet", "--target", rustHost, "-p", "kronika-registry", "--example", "ui_metadata"],
    { cwd: repository, encoding: "utf8", env: { ...process.env, CARGO_TERM_COLOR: "never" } },
  )
  const translations = await dictionaryModule(new URL("./", import.meta.url), reportMode ? ["export."] : [])
  const javascript = await bundleJavascript(registry, translations, fixtureOutput !== null, reportMode)
  const stylesheet = (await fontFaces()) + (await compileStylesheet(temporary, reportMode))
  const template = await readFile(join(uiDirectory, "src/index.html"), "utf8")
  const fixture = reportMode
    ? "/*KRONIKA_REPORT_RUNTIME*/"
    : fixtureOutput === null ? "" : await fixtureScript()
  const html = template
    .replaceAll("{{KRONIKA_STYLE}}", () => stylesheet)
    .replaceAll("{{KRONIKA_DATA}}", () => fixture)
    .replaceAll("{{KRONIKA_SCRIPT}}", () => javascript)

  validateHtml(html, reportMode)
  const compressed = Buffer.from(gzip(Buffer.from(html), { level: 9 }))
  validateGzipHeader(compressed)
  if (Buffer.byteLength(html) > maximumRawBytes || compressed.length > maximumGzipBytes) {
    throw new Error(
      `the UI exceeds its measured size bounds: raw ${Buffer.byteLength(html)}/${maximumRawBytes}, gzip ${compressed.length}/${maximumGzipBytes}`,
    )
  }

  if (checkOnly) {
    const committed = await readFile(artifact)
    if (!committed.equals(compressed)) {
      throw new Error("kronika-ui.html.gz differs from the reproducible build")
    }
  } else {
    await writeFile(artifact, compressed)
  }
  process.stdout.write(`kronika-ui${reportMode ? "-report" : ""} raw=${Buffer.byteLength(html)} gzip=${compressed.length}\n`)
} finally {
  await rm(temporary, { recursive: true, force: true })
}

async function bundleJavascript(registry, translations, includeFixture, reportMode) {
  const result = await build({
    absWorkingDir: uiDirectory,
    entryPoints: ["src/app.tsx"],
    bundle: true,
    charset: "utf8",
    define: {
      KRONIKA_REPORT: reportMode ? "true" : "false",
      "process.env.NODE_ENV": '"production"',
    },
    drop: ["console"],
    format: "iife",
    legalComments: "none",
    minify: true,
    platform: "browser",
    sourcemap: false,
    target: ["es2022"],
    write: false,
    plugins: [{
      name: "kronika-registry",
      setup(context) {
        if (reportMode) {
          context.onResolve({ filter: /^\.\/session$/ }, (args) => ({
            path: join(args.resolveDir, "report-session.ts"),
          }))
        }
        if (!includeFixture) {
          context.onResolve({ filter: /^\.\/fixture$/ }, () => ({
            path: "fixture.ts",
            namespace: "kronika-empty-fixture",
          }))
          context.onLoad({ filter: /.*/, namespace: "kronika-empty-fixture" }, () => ({
            contents: "export const bundledFixtureHeatmapRecords=()=>null;export const bundledFixtureHour=()=>null;export const bundledFixtureRange=()=>null",
            loader: "ts",
          }))
        }
        context.onResolve({ filter: /^kronika:registry$/ }, () => ({
          path: "registry.ts",
          namespace: "kronika",
        }))
        context.onLoad({ filter: /.*/, namespace: "kronika" }, () => ({
          contents: registry.replace(/"(columns|identity|logicalName|typeId)":/g, "$1:"),
          loader: "ts",
        }))
        context.onResolve({ filter: /^kronika:i18n$/ }, () => ({
          path: "i18n.ts",
          namespace: "kronika-i18n",
        }))
        context.onLoad({ filter: /.*/, namespace: "kronika-i18n" }, () => ({
          contents: translations,
          loader: "ts",
        }))
      },
    }],
  })
  const output = result.outputFiles[0]
  if (output === undefined) {
    throw new Error("esbuild produced no JavaScript")
  }
  return output.text
}

async function fontFaces() {
  const faces = await Promise.all(FONT_FILES.map(async ([family, weight, name, range]) => {
    const bytes = await readFile(join(uiDirectory, "assets", name))
    return `@font-face{font-family:"${family}";font-style:normal;font-weight:${weight};font-display:swap;`
      + `src:url(data:font/woff2;base64,${bytes.toString("base64")}) format("woff2");unicode-range:${range}}`
  }))
  return faces.join("")
}

async function fixtureScript() {
  const generated = join(temporary, "real-hour-with-heatmaps.json")
  execFileSync(
    "cargo",
    [`+${rustToolchain}`, "test", "--locked", "--quiet", "--target", rustHost, "-p", "kronika-web", "api::heatmap::real_fixture::export_real_hour_heatmaps", "--", "--ignored", "--exact"],
    {
      cwd: repository,
      stdio: "pipe",
      env: { ...process.env, CARGO_TERM_COLOR: "never", KRONIKA_REAL_HEATMAP_OUTPUT: generated },
    },
  )
  const raw = await readFile(generated, "utf8")
  JSON.parse(raw)
  const safe = raw.replaceAll("<", "\\u003c").replaceAll("\u2028", "\\u2028").replaceAll("\u2029", "\\u2029")
  return `globalThis.__KRONIKA_REAL_HOUR__=${safe};`
}

async function compileStylesheet(temporary, reportMode) {
  let input = join(uiDirectory, "src/styles.css")
  if (reportMode) {
    const exportStart = "/* KRONIKA_EXPORT_STYLES_BEGIN */"
    const exportEnd = "/* KRONIKA_EXPORT_STYLES_END */"
    const source = await readFile(input, "utf8")
    const start = source.indexOf(exportStart)
    const end = source.indexOf(exportEnd)
    if (start < 0 || end < start) throw new Error("export stylesheet markers are missing or out of order")
    const sourceDirectory = join(uiDirectory, "src").replaceAll("\\", "/")
    const tailwindStylesheet = join(uiDirectory, "node_modules/tailwindcss/index.css").replaceAll("\\", "/")
    const reportSource = (source.slice(0, start) + source.slice(end + exportEnd.length))
      .replace('@import "tailwindcss";', `@import "${tailwindStylesheet}" source(none);`)
      .replace('@source "./*.tsx";', `@source "${sourceDirectory}/*.tsx";\n@source not "${sourceDirectory}/export-dialog.tsx";`)
      .replace('@source "./clipboard.ts";', `@source "${sourceDirectory}/clipboard.ts";`)
    input = join(temporary, "kronika-report.css")
    await writeFile(input, reportSource)
  }
  const output = join(temporary, "kronika.css")
  execFileSync(
    join(uiDirectory, "node_modules/.bin/tailwindcss"),
    ["-i", input, "-o", output, "--minify"],
    { cwd: uiDirectory, stdio: "pipe" },
  )
  return readFile(output, "utf8")
}

function validateHtml(html, reportMode) {
  if (!html.startsWith("<!doctype html>")) {
    throw new Error("the built UI is not an HTML document")
  }
  const markers = ["{{KRONIKA_STYLE}}", "{{KRONIKA_DATA}}", "{{KRONIKA_SCRIPT}}"].filter((marker) => html.includes(marker))
  if (markers.length !== 0) {
    throw new Error(`the built UI contains unreplaced markers: ${markers.join(", ")}`)
  }
  if (/sourceMappingURL/i.test(html)) {
    throw new Error("source maps are forbidden in the production UI")
  }
  const reportMarkers = html.split("/*KRONIKA_REPORT_RUNTIME*/").length - 1
  if (reportMarkers !== (reportMode ? 1 : 0)) {
    throw new Error(`the UI has ${reportMarkers} report runtime markers`)
  }
  if (reportMode) {
    const serverOnly = [
      "/auth/session",
      "/api/export",
      "/api/mcp-access",
      "/api/instance-label",
      "fetch(",
      "XMLHttpRequest",
      "WebSocket",
      "EventSource",
      "sendBeacon",
      "X-Kronika-UI",
      "export.",
      "export-dialog",
      "export-trigger",
      "mcp-trigger",
      "refresh-action",
      "logout-action",
      "login-card",
    ].filter((needle) => html.includes(needle))
    if (serverOnly.length !== 0) {
      throw new Error(`the report UI contains server-only code: ${serverOnly.join(", ")}`)
    }
  }
  if (/\b(?:src|href)\s*=\s*["']\s*(?:https?:)?\/\//i.test(html)
      || /url\(\s*["']?\s*https?:\/\//i.test(html)) {
    throw new Error("the production UI contains an external asset URL")
  }
  const scriptStarts = scriptBodies(html).length
  const scriptEnds = html.toLowerCase().split("</script").length - 1
  if (scriptStarts !== 2 || scriptEnds !== 2) {
    throw new Error(`the production UI must contain two complete inline scripts (found ${scriptStarts}/${scriptEnds})`)
  }
}

function scriptBodies(html) {
  const bodies = []
  let tail = html
  for (;;) {
    const start = tail.indexOf("<script>")
    if (start < 0) return bodies
    const body = tail.slice(start + "<script>".length)
    const end = body.toLowerCase().indexOf("</script>")
    if (end < 0) throw new Error("the production UI contains an incomplete inline script")
    bodies.push(body.slice(0, end))
    tail = body.slice(end + "</script>".length)
  }
}

function validateGzipHeader(bytes) {
  if (bytes[0] !== 0x1f || bytes[1] !== 0x8b || bytes[2] !== 8 || bytes[3] !== 0) {
    throw new Error("gzip must use deflate without a stored name")
  }
  if (!bytes.subarray(4, 8).equals(Buffer.alloc(4))) {
    throw new Error("gzip modification time must be zero")
  }
}
