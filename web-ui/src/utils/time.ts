export function formatTime(t?: string | null) {
  if (!t) return "—";
  return new Date(t).toLocaleString();
}
