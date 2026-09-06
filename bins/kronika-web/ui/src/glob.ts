/** Matches a substring; `*` spans characters and `?` spans one character. */
export function globMatcher(pattern: string): ((text: string) => boolean) | null {
  const wanted = pattern.trim()
  if (wanted === "") return null
  const lowered = wanted.toLowerCase()
  if (!wanted.includes("*") && !wanted.includes("?")) {
    return (text) => text.toLowerCase().includes(lowered)
  }
  const expression = new RegExp([...lowered].map(escape).join(""))
  return (text) => expression.test(text.toLowerCase())
}

function escape(character: string): string {
  if (character === "*") return ".*"
  if (character === "?") return "."
  return /[a-z0-9\u0400-\u04ff_\s]/.test(character) ? character : `\\${character}`
}
