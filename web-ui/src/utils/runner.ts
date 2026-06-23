import type { RunnerConfig } from '../backend-types'

export type RunnerType = RunnerConfig['type']

export const RUNNER_LABELS: Record<RunnerType, string> = {
  shell: 'Shell',
  http: 'HTTP',
  pgSql: 'PostgreSQL',
  mySql: 'MySQL',
  python: 'Python',
  node: 'Node',
}

export function defaultRunner(type: RunnerType): RunnerConfig {
  switch (type) {
    case 'shell':
      return { type: 'shell', command: '', workingDir: null }
    case 'http':
      return {
        type: 'http',
        method: 'GET',
        url: '',
        headers: null,
        body: null,
        timeoutSec: null,
      }
    case 'pgSql':
      return { type: 'pgSql', configId: '', query: '', timeoutSec: null }
    case 'mySql':
      return { type: 'mySql', configId: '', query: '', timeoutSec: null }
    case 'python':
      return { type: 'python', module: '', className: '', timeoutSec: null }
    case 'node':
      return { type: 'node', module: '', functionName: '', timeoutSec: null }
  }
}

/** Whether the runner has the fields it needs to be submitted. */
export function isRunnerValid(cfg: RunnerConfig): boolean {
  switch (cfg.type) {
    case 'shell':
      return cfg.command.trim() !== ''
    case 'http':
      return cfg.url.trim() !== '' && cfg.method.trim() !== ''
    case 'pgSql':
    case 'mySql':
      return cfg.configId !== '' && cfg.query.trim() !== ''
    case 'python':
      return cfg.module.trim() !== '' && cfg.className.trim() !== ''
    case 'node':
      return cfg.module.trim() !== '' && cfg.functionName.trim() !== ''
  }
}
