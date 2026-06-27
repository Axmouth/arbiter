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

export function evictNode(nodeId: string): Promise<void> {
  return api<void>(`/node-keys/${nodeId}`, { method: 'DELETE' })
}

export function rotateKek(): Promise<RotateKekResponse> {
  return api<RotateKekResponse>('/secrets/rotate', { method: 'POST' })
}

/** Current rotation status, including the active KEK version (idle when none in flight). */
export function fetchRotationStatus(): Promise<RotateKekResponse> {
  return api<RotateKekResponse>('/secrets/rotation', { method: 'GET' })
}
