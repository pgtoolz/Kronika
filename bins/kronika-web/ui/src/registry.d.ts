declare module "kronika:registry" {
  export interface RegistryLayout {
    readonly typeId: string
    readonly logicalName: string | null
    readonly identity: readonly string[]
    readonly columns: readonly string[]
    readonly columnMetadata?: readonly {
      readonly name: string
      readonly type: string
      readonly class: "cumulative" | "gauge" | "label" | "timestamp"
      readonly unit: string | null
    }[]
  }

  export const registry: readonly RegistryLayout[]
}

declare module "*.css" {}
