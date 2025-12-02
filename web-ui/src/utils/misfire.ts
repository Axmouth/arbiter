import type { MisfirePolicy } from "../backend-types/MisfirePolicy";

export function misfirePolicyLabel(policy: MisfirePolicy): string {
  if (policy === "skip") return "Skip";
  if (policy === "run_immediately") return "Run Immediately";
  if (policy === "coalesce") return "Coalesce";
  if (policy === "run_all") return "Run All";

  if ("run_if_late_within" in policy) {
    const [secs, nanos] = policy.run_if_late_within;
    return `Run if late (≤ ${secs + Math.round(nanos / 1_000_000_000)}s)`;
  }

  return "Unknown";
}

export function misfirepolicyFromLabel(label: string): MisfirePolicy {
  if (label === "Skip") return "skip";
  if (label === "Run Immediately") return "run_immediately";
  if (label === "Coalesce") return "coalesce";
  if (label === "Run All") return "run_all";

  const runIfLateMatch = label.match(/^Run if late \(≤ (\d+)s\)$/);
  if (runIfLateMatch) {
    const secs = parseInt(runIfLateMatch[1], 10);
    return { run_if_late_within: [secs, 0] };
  }

  throw new Error(`Unknown misfire policy label: ${label}`);
}

export function inferMisfireType(mp: MisfirePolicy): string {
  if (mp === "skip") return "skip";
  if (mp === "run_immediately") return "run_immediately";
  if (mp === "coalesce") return "coalesce";
  if (mp === "run_all") return "run_all";
  if (typeof mp === "object" && "run_if_late_within" in mp) return "run_if_late_within";
  return "run_immediately";
}

export function inferMisfireDuration(mp: MisfirePolicy): number {
  if (typeof mp === "object" && "run_if_late_within" in mp) {
    return mp.run_if_late_within[0]; // seconds presumably
  }
  return 0;
}
