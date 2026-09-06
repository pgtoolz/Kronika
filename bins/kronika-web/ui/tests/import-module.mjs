import { Buffer } from "node:buffer"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"

import { build } from "esbuild"

const directory = dirname(fileURLToPath(import.meta.url))

export function registryPlugin(registry) {
  return {
    name: "registry",
    setup(context) {
      context.onResolve({ filter: /^kronika:registry$/ }, () => ({ namespace: "registry", path: "registry" }))
      context.onLoad({ filter: /.*/, namespace: "registry" }, () => ({
        contents: `export const registry=${JSON.stringify(registry)}`,
      }))
    },
  }
}

export async function importModule(contents, { loader = "tsx", plugins = [] } = {}) {
  const compiled = await build({
    bundle: true,
    format: "esm",
    platform: "node",
    plugins,
    stdin: { contents, loader, resolveDir: directory },
    treeShaking: true,
    write: false,
  })
  return import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].contents).toString("base64")}`)
}

export async function importFile(relativePath, options = {}) {
  return importModule(`export * from ${JSON.stringify(relativePath)}`, options)
}
