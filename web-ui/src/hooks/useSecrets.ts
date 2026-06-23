import { useQuery } from '@tanstack/react-query'
import { fetchSecrets } from '../api/secrets'

export function useSecrets() {
  return useQuery({
    queryKey: ['secrets'],
    queryFn: fetchSecrets,
    refetchInterval: 30000,
  })
}
