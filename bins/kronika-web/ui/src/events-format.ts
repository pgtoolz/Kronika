import type { Translate } from "./help"

const ERROR_CATEGORIES = [
  "events.category.lock", "events.category.constraint", "events.category.serialization",
  "events.category.timeout", "events.category.resource", "events.category.data_corruption",
  "events.category.system", "events.category.connection", "events.category.auth",
  "events.category.syntax", "events.category.other",
] as const

export function categoryLabel(category: number, t: Translate): string {
  const key = ERROR_CATEGORIES[category]
  return key === undefined ? String(category) : t(key)
}
