import type { ListRunsQuery } from "../backend-types";
import type { JobRun } from "../backend-types/JobRun";
import { api } from "./client";

export function fetchRuns(query: ListRunsQuery): Promise<JobRun[]> {
  return api<JobRun[]>("/runs", {
    method: "GET",
    headers: { "Content-Type": "application/json" },
  }, query as Record<string, string>);
}

export function cancelRun(id: string): Promise<void> {
  return api<void>(`/runs/${id}/cancel`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
  });
}