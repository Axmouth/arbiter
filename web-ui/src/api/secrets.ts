import type { CreateSecretRequest, SecretMetaResponse } from '../backend-types'
import { api } from './client'

export function fetchSecrets(): Promise<SecretMetaResponse[]> {
  return api<SecretMetaResponse[]>('/secrets')
}

export function createSecret(
  req: CreateSecretRequest
): Promise<SecretMetaResponse> {
  return api<SecretMetaResponse>('/secrets', {
    method: 'POST',
    body: JSON.stringify(req),
  })
}

export function deleteSecret(id: string): Promise<void> {
  return api<void>(`/secrets/${id}`, { method: 'DELETE' })
}
