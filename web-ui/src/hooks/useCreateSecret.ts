import { useMutation, useQueryClient } from '@tanstack/react-query'
import { createSecret } from '../api/secrets'
import type { CreateSecretRequest } from '../backend-types'

// Shared create-secret mutation so every entry point (the Secrets page and the
// inline picker) writes through one path and refreshes the secrets list the same
// way. Callers add their own onSuccess via mutate(vars, { onSuccess }).
export function useCreateSecret() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (req: CreateSecretRequest) => createSecret(req),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['secrets'] }),
  })
}
