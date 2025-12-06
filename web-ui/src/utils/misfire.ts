import type { MisfirePolicy } from "../backend-types/MisfirePolicy";

export function misfirePolicyLabel(policy: MisfirePolicy): string {
  if (policy === "skip") return "Skip";
  if (policy === "runImmediately") return "Run Immediately";
  if (policy === "coalesce") return "Coalesce";
  if (policy === "runAll") return "Run All";

  if ("runIfLateWithin" in policy) {
    const [secs, nanos] = policy.runIfLateWithin;
    return `Run if late (≤ ${secs + Math.round(nanos / 1_000_000_000)}s)`;
  }

  return "Unknown";
}

export function misfirepolicyFromLabel(label: string): MisfirePolicy {
  if (label === "Skip") return "skip";
  if (label === "Run Immediately") return "runImmediately";
  if (label === "Coalesce") return "coalesce";
  if (label === "Run All") return "runAll";

  const runIfLateMatch = label.match(/^Run if late \(≤ (\d+)s\)$/);
  if (runIfLateMatch) {
    const secs = parseInt(runIfLateMatch[1], 10);
    return { runIfLateWithin: [secs, 0] };
  }

  throw new Error(`Unknown misfire policy label: ${label}`);
}

export function inferMisfireType(mp: MisfirePolicy): string {
  if (mp === "skip") return "skip";
  if (mp === "runImmediately") return "run_immediately";
  if (mp === "coalesce") return "coalesce";
  if (mp === "runAll") return "run_all";
  if (typeof mp === "object" && "runIfLateWithin" in mp) return "runIfLateWithin";
  return "run_immediately";
}

export function inferMisfireDuration(mp: MisfirePolicy): number {
  if (typeof mp === "object" && "runIfLateWithin" in mp) {
    return mp.runIfLateWithin[0]; // seconds presumably
  }
  return 0;
}
