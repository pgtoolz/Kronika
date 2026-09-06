const SUMMARY_LENGTH = 84

export function planTextSummary(plan: string | null): string | null {
  if (plan === null) return null
  const firstLine = plan.split(/\r?\n/).find((line) => line.trim() !== "")?.trim()
  if (firstLine === undefined) return null
  return firstLine.length > SUMMARY_LENGTH ? `${firstLine.slice(0, SUMMARY_LENGTH - 1)}…` : firstLine
}
