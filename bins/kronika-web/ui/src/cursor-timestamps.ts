export interface Timestamped {
  readonly timestamp: number
}

export function observationTimestamps(...sources: readonly (readonly Timestamped[])[]): readonly number[] {
  return mergeObservationTimestamps([], ...sources)
}

export function mergeObservationTimestamps(
  base: readonly number[],
  ...sources: readonly (readonly Timestamped[])[]
): readonly number[] {
  const timestamps = new Set<number>()
  for (const timestamp of base) {
    if (Number.isSafeInteger(timestamp)) timestamps.add(timestamp)
  }
  for (const source of sources) {
    let previousTimestamp: number | undefined
    for (const observation of source) {
      const { timestamp } = observation
      if (timestamp !== previousTimestamp && Number.isSafeInteger(timestamp)) timestamps.add(timestamp)
      previousTimestamp = timestamp
    }
  }
  return [...timestamps].sort((left, right) => left - right)
}
