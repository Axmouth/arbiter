import { useQuery } from "@tanstack/react-query";
import { fetchRuns } from "../api/runs";

export function useRuns() {
  return useQuery({
    queryKey: ["runs"],
    queryFn: () => fetchRuns({}),
    refetchInterval: 3000, // auto refresh
  });
}

