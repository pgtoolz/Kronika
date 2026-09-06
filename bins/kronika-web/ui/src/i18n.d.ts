declare module "kronika:i18n" {
  export type TranslationKey = string
  export function translation(locale: "en" | "ru", key: string): string | undefined
}
