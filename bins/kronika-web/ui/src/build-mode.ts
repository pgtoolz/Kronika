export function historyAddress(address: string, pathname: string, reportMode = KRONIKA_REPORT): string {
  if (!reportMode) return address
  const query = address.indexOf("?")
  return query < 0 ? pathname : `${pathname}${address.slice(query)}`
}
