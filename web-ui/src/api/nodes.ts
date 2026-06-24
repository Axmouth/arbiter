import type { NodeKeyResponse, RotateKekResponse } from '../backend-types'
import { api } from './client'

export function fetchNodeKeys(): Promise<NodeKeyResponse[]> {
  return api<NodeKeyResponse[]>('/node-keys')
}

export function approveNode(nodeId: string): Promise<void> {
  return api<void>(`/node-keys/${nodeId}/approve`, { method: 'POST' })
}

export function revokeNode(nodeId: string): Promise<void> {
  return api<void>(`/node-keys/${nodeId}/revoke`, { method: 'POST' })
}

export function rotateKek(): Promise<RotateKekResponse> {
  return api<RotateKekResponse>('/secrets/rotate', { method: 'POST' })
}
