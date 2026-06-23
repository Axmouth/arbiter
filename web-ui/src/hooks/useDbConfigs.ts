import { useQuery } from '@tanstack/react-query'
import { fetchDbConfigs } from '../api/configs'

export function useDbConfigs() {
  return useQuery({
    queryKey: ['db-configs'],
    queryFn: fetchDbConfigs,
    refetchInterval: 30000,
  })
}
