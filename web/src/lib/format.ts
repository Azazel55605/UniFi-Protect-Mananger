/** Byte sizes, in the units a person reading a storage page thinks in. */
export function formatBytes(bytes: number, decimals?: number) {
  const abs = Math.abs(bytes);
  if (abs < 1024) return `${Math.round(bytes)} B`;
  if (abs < 1024 ** 2) return `${(bytes / 1024).toFixed(decimals ?? 0)} KB`;
  if (abs < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(decimals ?? 1)} MB`;
  if (abs < 1024 ** 4) return `${(bytes / 1024 ** 3).toFixed(decimals ?? 1)} GB`;
  return `${(bytes / 1024 ** 4).toFixed(decimals ?? 2)} TB`;
}

/** A duration in days, phrased at the precision it deserves. */
export function formatDays(days: number) {
  if (!Number.isFinite(days)) return "—";
  if (days < 1) return "under a day";
  if (days < 90) return `${Math.round(days)} days`;
  if (days < 730) return `${Math.round(days / 30)} months`;
  return `${(days / 365).toFixed(1)} years`;
}
