import { useQuery } from "@tanstack/react-query";
import type { WorkerRecord } from "../backend-types/WorkerRecord";
import { fetchWorkers } from "../api/workers";

export function useWorkers() {
  return useQuery<WorkerRecord[]>({
    queryKey: ["workers"],
    queryFn: fetchWorkers,
    refetchInterval: 5000, // keep worker statuses fresh
  });
}
