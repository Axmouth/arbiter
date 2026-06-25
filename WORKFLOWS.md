# Arbiter workflows — brainstorm notes

> Status: **BRAINSTORM, not a spec.** A capture of the direction for a workflow/orchestration
> layer on top of arbiter's jobs+runs. Nothing here is built or committed to; it is raw
> thinking to refine in a later design session. Items are grouped from a stream-of-thought
> dump, preserving intent. Where it leans on something that already exists, that is noted.

## 1. Core model

- **Workflows chain jobs** into a graph of steps. A step runs a job (one of the existing
  runner kinds) and feeds the next.
- **A workflow can itself be treated as a job** in some contexts — i.e. workflows are
  composable/nestable; a step could be another workflow. ("in some contexts a workflow can be
  treated as a job too.")

## 2. Data flow between steps

- **Result pointers:** a step can "query" a previous step's result by a **format-aware
  pointer** — JSON (JSONPath-ish), XML (XPath-ish), etc. — with **regex match as a pinch
  fallback**. Builds on the existing structured-result protocol (`result` +
  `result_media_type`), which already gives typed per-step output.
  - **Pointer language (more advanced than a plain path):**
    - **List indexing**, including **negative / backward indexing** (e.g. last element).
    - **Wildcard over a list = fan-out**: a wildcard selects every element and interpolates
      the pointer per element, driving the fan-out (each element becomes its own branch/run).
    - **Stacked wildcards to go deeper**: more wildcards descend into nested lists (operate at
      increasing list depth as needed).
    - **A single nested wildcard flattens** one level of nesting (flatten-map semantics).
    - **Pending: object wildcards** — design a similar wildcard/iterate ability over object
      keys/values, not just lists (TBD).
- **Jsonify non-JSON output:** be able to turn things like DB query rows into JSON so they are
  addressable by pointers in the workflow. (Ties to "analyze a query's output type" below.)
- **Shared state:** a step can **publish to a shared workflow state** that later steps read.
  A workflow-scoped key/value/document the graph can accumulate into.

## 3. Control flow

- **Branch on success or error** — different next step depending on the step's outcome
  (builds on the existing `result_status` success/failed/retryable classification).
- **Conditional branches** — branch on a predicate over a step's result / shared state.
- **Multiple branches from one step** — fan to several next steps; conditions optional (so a
  plain multi-successor split is allowed, not only condition-gated).
- **Loop back to a previous step** — cycles in the graph (with some bound/guard, TBD).
- **Fan-out from list output** — map a step over each item of a previous step's list result
  (parallel per-item execution; relates to partitioning/parallelism already in the model).

## 4. Schemas & validation

- **Infer output type where the runner allows it:** e.g. for DB query steps, analyze the
  query and derive the result's **JSON schema**, and save it.
- **Declare/validate a JSON schema** for a step's output in the workflow; validate at runtime.
- **A schema-definition helper** (UI) to make defining schemas easy.
- **Schema drift detection:** runs record the **observed** schema of a step's response and
  **warn when it is inconsistent** with the declared one — with a one-click action to **accept
  arbiter's refined schema** (the one derived from actual runs) as the new definition.

## 5. Definition format (export/import)

- Workflows are **declarative**, with **export/import in JSON / TOML / XML** — one canonical
  internal model, just **different renderers/parsers** per format. (Parallels the
  "canonical grand-JSON" cross-backend idea already noted in FOLLOWUPS.)

## 6. Durability / resumability

- **Checkpoint workflow state at each step** so a workflow is **pausable and continuable** —
  resume from where it stopped rather than re-running from the top. (The rotation state
  machine and the run lifecycle are precedents for resumable, store-backed progress.)

## Open questions / things to pin down later

- Where the workflow definition + run state live (new tables vs. reuse `job_runs` per step);
  must stay distributed (in the Store), like everything else.
- Cycle/loop guards (max iterations, timeouts) to keep loops from running away.
- How shared state is scoped, sized, and persisted (and secret-safe — no plaintext leaks).
- Fan-out concurrency limits and partial-failure semantics (one item fails — branch? collect?).
- Pointer language: adopt JSONPath/XPath vs. a small in-house selector; regex as the fallback.
- How "a workflow is also a job" maps onto the scheduler/claim model (a workflow-runner kind?).
- Relationship to the **chain jobs** and **notifications/webhooks** parity items already noted.
