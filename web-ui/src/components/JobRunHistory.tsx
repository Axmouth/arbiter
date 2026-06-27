import type { JobRun } from '../backend-types/JobRun'
import { Table, THead, Th, TBody, Tr, Td } from './Table'
import { Badge } from './Badge'
import { formatTime } from '../utils/time'

export function JobRunHistory({
  runs,
  onSelect,
}: {
  runs: JobRun[]
  onSelect?: (run: JobRun) => void
}) {
  if (runs.length === 0) {
    return <p className="text-sm text-(--text-muted)">No runs recorded yet.</p>
  }

  return (
    <Table>
      <THead>
        <Th>State</Th>
        <Th>Scheduled</Th>
        <Th>Started</Th>
        <Th>Finished</Th>
      </THead>
      <TBody>
        {runs.map((run) => (
          <Tr key={run.id} onClick={onSelect ? () => onSelect(run) : undefined}>
            <Td>
              <Badge
                tone={
                  run.state === 'succeeded'
                    ? 'success'
                    : run.state === 'failed'
                      ? 'error'
                      : 'neutral'
                }
              >
                {run.state}
              </Badge>
            </Td>
            <Td>{formatTime(run.scheduledFor)}</Td>
            <Td>{formatTime(run.startedAt)}</Td>
            <Td>{formatTime(run.finishedAt)}</Td>
          </Tr>
        ))}
      </TBody>
    </Table>
  )
}
