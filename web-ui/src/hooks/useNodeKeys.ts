import { useQuery } from '@tanstack/react-query'
import { fetchNodeKeys } from '../api/nodes'

export function useNodeKeys() {
  return useQuery({
    queryKey: ['node-keys'],
    queryFn: fetchNodeKeys,
    refetchInterval: 10000,
  })
}
