import { useQuery } from '@tanstack/react-query'
import { fetchTenants } from '../api/tenants'

export function useTenants() {
  return useQuery({
    queryKey: ['tenants'],
    queryFn: fetchTenants,
    refetchInterval: 30000,
  })
}
