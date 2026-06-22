# Runner result protocol -- precedent study and proposed design

Status: research / proposal (pre-implementation). Tracks FOLLOWUPS §3a.

## Problem

A run today yields only `exit_code` + raw `stdout` + `stderr`. That cannot separate a
structured **return value** from incidental logging, a **typed error** (class / message /
stack) from a generic non-zero exit, a **retryable** failure from a permanent one, **logs**
(a leveled, timestamped stream) from **output** (the result), or carry **progress /
heartbeat** for long tasks. We want a richer, opt-in contract without losing the universal
"run any script or binary" floor.

## Precedent survey

| System | Channel | Result / error shape | Notable lessons |
|---|---|---|---|
| **Cronicle** (closest comparable) | NDJSON on **stdout**; non-JSON stdout/stderr auto-appended to the log | `{complete:1, code, description}` for done; `progress` 0..1; `perf` metrics; `table`/`html` reports; `chain`/`chain_error` for DAGs; `label`; `update_event` | Rich and proven. But it overloads stdout, so it needs an "Interpret JSON in Output" toggle and a "last JSON line wins" rule -- the channel is ambiguous by construction. |
| **GitHub Actions** | Migrated **off** stdout (`::set-output::`) **to a file** via `$GITHUB_OUTPUT` (append `key=value`); logging/annotations still use `::error file=,line=::msg` on stdout | outputs are key/value; annotations carry file/line | The deprecation of stdout `set-output` for a file env-var is the single most relevant lesson: stdout command-injection / interleaving is fragile and unsafe. Prefer a dedicated file. |
| **systemd `sd_notify`** | Out-of-band **datagram socket** at `$NOTIFY_SOCKET` | `READY=1`, `STATUS=<text>`, `WATCHDOG=1` keep-alive; extensions prefixed `X_` | Liveness / progress belongs on a side channel, not the result. The watchdog maps directly to our reaper: a long task that heartbeats should not be reclaimed. |
| **AWS Lambda Runtime API** | **HTTP** to the runtime endpoint | separate `/response` vs `/error`; error = `{errorMessage, errorType, stackTrace[]}` | Explicit, separate success-vs-error reporting; a concrete structured-error shape worth copying. |
| **Exit-code conventions** (`sysexits.h`) | the exit code itself | `EX_TEMPFAIL = 75` = "temp failure, retry later" (used by sendmail/postfix to requeue) | A retryable signal is possible even with zero protocol -- but it is a single ambiguous integer, so promote it to an explicit `status` while still honoring it as a fallback. |
| **Airflow XCom** | return value pushed to the metadata store | small return values; size-limited | Return value as first-class output, but **bounded** -- large data goes to external storage, not the result row. |
| **Temporal** activities | SDK return value / typed exception | `ApplicationFailure` carries a `nonRetryable` flag + type + details | Make retryability an explicit property of the failure, set by user code, not inferred. |
| **Nomad** task drivers | separate stdout/stderr FIFOs via `logmon` | -- | Keep stdout and stderr as independent free-form log streams; do not multiplex control data through them. |
| **dbt** | `run_results.json` artifact file | structured per-node status/timing | The "write a structured artifact file the orchestrator reads afterward" pattern, again file-based. |

## Distilled principles

1. **Dedicated channel, never stdout.** Every system that started on stdout (Cronicle's
   toggle, GitHub's deprecation) regretted the ambiguity. Use a file whose path is handed
   to the child via env. Keep stdout/stderr as free-form captured logs (Cronicle/Nomad).
2. **Separate result from liveness.** Final result is one thing; progress/heartbeat is a
   side stream (sd_notify). Heartbeat must feed the reaper so long jobs are not reclaimed.
3. **Status is explicit and tri-state.** `success | failed | retryable`, set by the task,
   not inferred from an integer (Temporal). Still honor exit codes as a fallback.
4. **Structured, typed errors.** `{type, message, stack}` (Lambda), not a stderr blob.
5. **Output is bounded JSON; big data goes elsewhere.** (Airflow XCom). Output is the
   structured return value, not a dumping ground.
6. **Version the contract.** Carry `protocolVersion` in both the handshake (env) and the
   result; reserve an extension namespace (sd_notify `X_`).
7. **Opt-in with a graceful floor.** If the child writes nothing, fall back to exit-code +
   stdout/stderr semantics (layer (a)). The protocol never breaks "run any binary".

## Chosen design: an injected language-side runtime

Cold subprocess (isolation, hard timeout via kill, real concurrency, the user's own
interpreter + deps) -- but the worker does not invoke the user's code directly. It invokes
a thin **runtime/harness written in that language** that imports the user's callable, runs
it, marshals the return value by type, captures errors and logs, and **owns the
transport**. Three layers:

- **Layer C -- user code:** `run(ctx) -> X` (and optional `prepare(ctx)`). Never sees a
  file, socket, or wire format.
- **Layer B -- the runtime** (`arbiter_runtime.py`, `arbiter_runtime.js`): import, run,
  marshal, error/log handling, transport. Thin and dependency-free (stdlib only).
- **Layer A -- the worker<->runtime wire contract:** transport-agnostic messages
  (`recv_task`, `send_event`, `send_result`).

Because Layer B owns the transport behind an interface, switching file -> unix socket ->
websocket touches only the worker and the runtime; **user code is untouched**. This is the
design's main leverage, and it is what unlocks prearm (below).

### Vendored, zero-install (now)
The v1 runtime is a single **stdlib-only file** -- no `pip`/`npm` install to try it:
```
# before (raw):   python3 -c "from mytask import MyTask; MyTask()"
# now (runtime):  python3 <runtime> --module mytask --entry run --result-file <tmp>
```
The worker writes the runtime **once** to a content-addressed path
(`<tempdir>/arbiter-runtime/arbiter_runtime_<hash>.py`) and reuses it across runs; the
hash in the name auto-invalidates on any runtime edit, and the write is atomic (temp +
rename) so a first-write race cannot expose a partial file. The runtime imports the module
(resolved via the `PYTHONPATH`/`NODE_PATH` we inject as the job's env), calls the
entrypoint, and writes the result. A published pip/npm package comes later for the
resident/prearm mode; the raw `-c` path can return as a no-runtime fallback.

### Handshake on argv (env stays the user's)
The control handshake travels on **argv**, not env, so we never pollute the user's process
environment (which carries only their own vars, e.g. `PYTHONPATH`) and nothing leaks to
grandchild processes the task may spawn:
```
--module M  --entry E (default run)  --result-file PATH  --run-id ID
--transport file  --protocol N
```
Rationale: the handshake is non-sensitive and fixed-size, so argv is clean; **secrets** are
a *separate* payload that will travel in a `0600` file or over the socket (P2/§13), never
argv/env. A larger payload can later move behind `--input <file>` without changing the
contract.

### Transport
- `file` (v1): result written to the `--result-file` path; that file is a `tempfile` whose
  `TempPath` is deleted on drop (cleanup owned by the worker, "upstairs" -- the child never
  deletes it, since the worker must read it after the child exits and the child may be
  killed before any self-cleanup could run). A worker-side sweep GCs crash-orphaned result
  files (follow-up).
- `socket` (later): same messages, duplex, over a socket path -- required for resident mode
  and event streaming. Per-task params then arrive as socket messages (argv/env are
  spawn-time only), which is why neither argv nor a config file "carries forward" -- the
  durable shape is a message.

### Result document (`ARBITER_RESULT_FILE`)
```json
{
  "protocolVersion": 1,
  "status": "success | failed | retryable",
  "output": <any bounded json return value>,
  "error":  { "type": "...", "message": "...", "stack": ["..."] }
}
```
Marshaling rule (Python): pydantic `.model_dump()` if present, else `dataclasses.asdict`,
else json-able as-is, else public `__dict__`, else `str`. Node: value as-is via
`JSON.stringify`; `Error` -> `{type,message,stack}`.

### Lifecycle and modes
- `prepare(ctx)` -- prearm hook (warm imports, DB pools, auth). `run(ctx)` -- fire time.
- **One-shot (file, v1):** spawn -> `prepare?()` -> `run()` -> result -> exit. `prepare`
  runs inline (no real prearm yet).
- **Resident (socket, later):** worker spawns the runtime *ahead* of fire time; it runs
  `prepare()` then idles on the open socket; at fire time the worker sends `run` and gets
  the result back, staying warm for the next fire. Same Layer B/C code, duplex transport.
  This *is* prearm.

### Resolution / fallback (worker, after the child exits)
1. Valid result file -> use its `status` / `output` / `error`.
2. No result file -> fall back to exit code (`0` success; later `75` EX_TEMPFAIL ->
   retryable) + captured stdout/stderr.
3. stdout/stderr are always captured as the run's logs. The result never rides stdout.

### Data-model (implemented)
Outcome is **text + media type**, not forced JSON. `job_runs` carries the universal text
streams `stdout`/`stderr`, the typed payloads `result`/`result_media_type` and
`error`/`error_media_type`, plus `result_status` (success|failed|retryable, distinct from
`exit_code`) and `attempt`. So shell fills `stdout`/`stderr`; http fills `result` + the
response `Content-Type`; the runtime fills `result` (return value: `application/json`, or
`text/plain` for a bare string) and a structured `error` (`application/json`). Unified to
TEXT on both backends (resolves §6; PG `output` JSONB dropped). Worker maps each runner to
a `RunOutcome`; `finalize_run` records terminal outcomes, `reschedule_for_retry` requeues.

### Retry (implemented)
Per-job `max_attempts` (default 1 = none) + `backoff_strategy` (fixed | exponential |
fibonacci) + `backoff_base_secs` + `backoff_cap_secs`, with **mandatory full jitter**
(`core::next_retry_delay`). A `retryable` outcome while attempts remain requeues the run
with the computed backoff; otherwise it fails. Retryable sources: runtime status (future:
explicit `Retryable`), HTTP 408/425/429/5xx + transport errors, shell `exit 75`
(EX_TEMPFAIL).

## Phasing
- **P1 (done):** injected vendored runtimes (python/node), `file` transport + versioned
  result schema, argv handshake, reused content-addressed runtime, `prepare`/`run`
  lifecycle (`prepare` inline). Conformance + full-flow.
- **P2 data model (done):** structured outcome columns (text + media type) + per-job retry
  with jittered backoff strategies. Conformance `outcome::*`/`retry::*`, full-flow retry.
- **P2 remainder (planned):** `socket` transport + resident mode -> prearm; event stream
  (`ARBITER_EVENTS_FILE`) for logs/progress/heartbeat + `last_heartbeat`/`progress` columns
  + reaper heartbeat.
- **P3:** published pip/npm SDK packages; richer `ctx` (params, secrets, artifacts).

## Sources
- Cronicle plugin protocol: https://github.com/jhuckaby/Cronicle/blob/master/docs/Plugins.md
- GitHub Actions workflow commands / `$GITHUB_OUTPUT`: https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-commands
- systemd `sd_notify`: https://www.freedesktop.org/software/systemd/man/latest/sd_notify.html
- AWS Lambda runtime API: https://docs.aws.amazon.com/lambda/latest/dg/runtimes-api.html
- `sysexits.h` (EX_TEMPFAIL): https://www.man7.org/linux//man-pages/man3/sysexits.h.3head.html
