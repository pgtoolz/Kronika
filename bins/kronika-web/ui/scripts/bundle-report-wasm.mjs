import { readFile, writeFile } from "node:fs/promises"
import { dirname, resolve } from "node:path"

import { build } from "esbuild"

const [inputArgument, outputArgument, ...extra] = process.argv.slice(2)
if (inputArgument === undefined || outputArgument === undefined || extra.length !== 0) {
  throw new Error("usage: node scripts/bundle-report-wasm.mjs INPUT OUTPUT")
}

const input = resolve(inputArgument)
const output = resolve(outputArgument)
const generated = await readFile(input, "utf8")
const initMarker = "function initSync(module) {"
const exportMarker = "export { initSync, __wbg_init as default };"
if (occurrences(generated, initMarker) !== 1 || occurrences(generated, exportMarker) !== 1) {
  throw new Error("wasm-bindgen output does not contain the expected initialization markers")
}
const embeddedInitializer = `async function initEmbedded(module) {
    if (wasm !== undefined) return wasm;
    const imports = __wbg_get_imports();
    const instance = await WebAssembly.instantiate(module, imports);
    return __wbg_finalize_init(instance, module);
}

`
const prepared = generated
  .replace(initMarker, `${embeddedInitializer}${initMarker}`)
  .replace(exportMarker, "export { initEmbedded, initSync, __wbg_init as default };")
const result = await build({
  bundle: true,
  charset: "utf8",
  format: "iife",
  globalName: "KronikaReportWasm",
  legalComments: "none",
  logOverride: { "empty-import-meta": "silent" },
  minify: true,
  platform: "browser",
  sourcemap: false,
  stdin: {
    contents: "export { initEmbedded, ReportSession } from './kronika-report-wasm.js'",
    loader: "js",
    resolveDir: dirname(input),
    sourcefile: "kronika-report-wasm-entry.js",
  },
  target: ["es2022"],
  treeShaking: true,
  write: false,
  plugins: [{
    name: "kronika-embedded-wasm",
    setup(context) {
      context.onLoad({ filter: /kronika-report-wasm\.js$/ }, (args) => (
        resolve(args.path) === input ? { contents: prepared, loader: "js" } : undefined
      ))
    },
  }],
})
const bundled = result.outputFiles[0]
if (bundled === undefined) throw new Error("esbuild produced no report bindings")

await writeFile(output, bundled.contents)

function occurrences(text, needle) {
  return text.split(needle).length - 1
}
