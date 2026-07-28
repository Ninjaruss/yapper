// Single mm:ss formatter for the whole app (elapsed clock, transcript
// stamps, recap durations, player time). Clamps negatives to 0:00 so a
// stray negative never renders as "-1:-3".
export function fmtDuration(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}

export function fmtDate(ms: number): string {
  return new Date(ms).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}
