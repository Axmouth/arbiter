# Arbiter workflows — brainstorm notes

> Status: **BRAINSTORM, not a spec.** A capture of the direction for a workflow and
> orchestration layer on top of arbiter's jobs and runs. Nothing here is built or committed
> to. It is raw thinking to refine in a later design session. Items are grouped from a
> stream-of-thought dump, preserving intent. Where it leans on something that already exists,
> that is noted.

## 1. Core model

- **Workflows chain jobs** into a graph of steps. A step runs a job (one of the existing
  runner kinds) and feeds the next.
- **A workflow can itself be treated as a job** in some contexts. Workflows are composable and
  nestable, so a step could be another workflow.

## 2. Data flow between steps

- **Result pointers:** a step can "query" a previous step's result by a format-aware pointer.
  JSON (JSONPath-ish), XML (XPath-ish), and so on, with regex match as a pinch fallback. This
  builds on the existing structured-result protocol (`result` plus `result_media_type`), which
  already gives typed per-step output.
  - **Reuse an existing expression language** (for example JMESPath or jq) rather than invent
    one, provided it supports the capabilities below. The capabilities are the hard
    requirement. The implementation should be borrowed, not hand-rolled, because homegrown
    selector DSLs are where these tools accrete subtle bugs. JMESPath in particular already has
    projections, `[]` flattening, slicing, and negative indexing, which covers most of this.
  - **Pointer capabilities required (more advanced than a plain path):**
    - **List indexing**, including negative or backward indexing (for example the last
      element).
    - **Wildcard over a list drives fan-out.** A wildcard selects every element and
      interpolates the pointer per element, so each element becomes its own branch or run.
    - **Stacked wildcards go deeper.** More wildcards descend into nested lists, operating at
      increasing list depth as needed.
    - **A single nested wildcard flattens** one level of nesting (flatten-map semantics).
    - **Pending: object wildcards.** Design a similar wildcard or iterate ability over object
      keys and values, not just lists. TBD.
- **Jsonify non-JSON output:** turn things like DB query rows into JSON so they are addressable
  by pointers in the workflow. This ties to "analyze a query's output type" below.
- **State and accumulation (the hard one, refined).** A single global mutable shared state is
  the trap. It is race-prone under fan-out and hard to reason about, and most workflow engines
  avoid it for exactly that reason. The refined direction is to make state flow **explicit and
  scoped** instead of ambient, which removes the concurrent-write surface entirely while still
  giving both loop accumulation and cross-step accumulation:
  - **Default: dataflow, not shared state.** Step outputs are immutable and addressable by
    pointer. Most "shared state" needs are really just reading a prior step's output, which the
    pointers already cover. No mutable blackboard required.
  - **Loop accumulators (sequential, so safe).** A loop declares an explicit accumulator that
    each iteration reads and updates, a fold: `acc = f(acc, iteration_output)`. A loop runs one
    iteration at a time, so this is a single writer over time with no race. It gives the
    "accumulate across iterations" capability cleanly, scoped to the loop rather than global.
  - **Fan-out reduce (parallel, so collect then fold).** Parallel branches do not write shared
    state. They return values, the parent collects them into a list, and a reduce step folds
    that list. Accumulation across parallel work is explicit collect-then-reduce, never a
    concurrent write.
  - **Shared state as a commutative accumulator (the good escape hatch).** A real
    concurrently-writable workflow state is safe if writes are restricted to **order-
    independent operations**, never an overwrite. This is the CRDT idea. The safe operation set
    is roughly:
    - **Numeric add and subtract (counters).** Addition and subtraction commute
      unconditionally, so concurrent fan-out writes converge regardless of order, with no
      special cases. This is the primary commutative accumulator.
    - **Collect.** Union-collect into a **set** is order-independent and safe under
      concurrency. Collect into an ordered **list** is *not*, since the order depends on
      interleaving, so ordered accumulation goes through collect-then-reduce or a sequential
      loop fold instead. (Set *removal* under concurrency is the one case that needs a proper
      CRDT set with tombstones, so leave it out of the MVP. Grow-only union is fine.)
    - This gives a genuine shared accumulator that parallel branches can write to safely, as
      long as the operation is commutative. It is more powerful than forbidding writes in
      branches, and it is race-free by construction rather than by discipline.
  - Net: no overwrite races anywhere. Loop accumulation via a scoped fold, parallel ordered
    accumulation via collect-then-reduce, and a real shared accumulator for the commutative
    cases (numeric add/sub and union-collect into a set).

## 3. Control flow

- **Branch on success or error.** A different next step depending on the step's outcome. This
  builds on the existing `result_status` success, failed, and retryable classification.
- **Conditional branches.** Branch on a predicate over a step's result or the shared state.
- **Multiple branches from one step.** Fan to several next steps, with conditions optional, so
  a plain multi-successor split is allowed and not only the condition-gated form.
- **Loop back to a previous step.** Cycles in the graph, with some bound or guard. TBD. A loop
  can carry an explicit accumulator (a fold across iterations), see the state section below.
- **Fan-out from list output.** Map a step over each item of a previous step's list result,
  running per-item in parallel. This relates to the partitioning and parallelism already in the
  model.
  - **Concurrency guard or limit.** Fan-out needs a cap (max parallel items or max items) so a
    huge list cannot blow up the cluster.
  - **Partial-failure policy (decided leaning).** By default it collects rather than aborts.
    The fan-out's output is two lists, `results` and `errors`, in the style of
    Promise.allSettled, so a downstream step can handle both. An opt-in short-circuit flag
    aborts the fan-out on the first failure for the cases that want fail-fast.

## 4. Schemas and validation

- **Infer output type where the runner allows it.** For example, for DB query steps, analyze
  the query, derive the result's JSON schema, and save it.
- **Declare and validate a JSON schema** for a step's output in the workflow, validated at
  runtime.
- **A schema-definition helper** in the UI to make defining schemas easy.
- **Schema drift detection.** Runs record the observed schema of a step's response and warn
  when it is inconsistent with the declared one, with a one-click action to accept arbiter's
  refined schema (the one derived from actual runs) as the new definition.

## 5. Definition format (export and import)

- Workflows are declarative, with export and import in JSON, TOML, and XML. One canonical
  internal model, with different renderers and parsers per format. This parallels the canonical
  grand-JSON cross-backend idea already noted in FOLLOWUPS.

## 6. Durability and resumability

- **Checkpoint workflow state at each step** so a workflow is pausable and continuable, able to
  resume from where it stopped rather than re-running from the top. The rotation state machine
  and the run lifecycle are precedents for resumable, store-backed progress.

## Assessment and decisions so far

Honest read: the direction is sound and cheaper than it looks, because it mostly composes
existing primitives (structured results, `result_status`, retry, durable runs, runners, and the
distributed store) rather than building an engine from scratch. "A workflow is also a job"
reuses the scheduler and claim machinery. The real risk is scope and sequencing, plus two
specific traps flagged in review. Decisions and leanings captured so far:

- **DSL: borrow, do not invent.** The capabilities are required. JMESPath or jq is the likely
  base.
- **Fan-out partial failure: collect by default** (two lists, `results` and `errors`), with an
  opt-in short-circuit and a concurrency cap.
- **Shared state under fan-out: lean (a).** Read-only in parallel branches, collect children to
  a list, and write shared state only on the linear path. Map, collect, reduce, and race-free.
- **Schema inference is kept as the "okay to wow" differentiator.** The earlier "non-trivial"
  note was about sequencing (do it after the spine), not about cutting it.

Danger status (honest tally of the risks raised in review):

- **Scope explosion:** addressed by the MVP-spine sequencing below (a plan, not yet executed).
- **Pointer DSL tarpit:** addressed. Borrow JMESPath or jq, invent nothing.
- **Fan-out partial failure:** addressed. Collect two lists by default, opt-in short-circuit,
  concurrency cap.
- **Shared-state races:** addressed by the explicit-dataflow and scoped-accumulator model
  above. There are no concurrent writes anywhere.
- **Schema inference ambition:** addressed by sequencing it last while keeping it.
- **Still open: resume and determinism.** Checkpoint and resume is only correct if a resumed
  workflow never re-executes a committed step. Each step's output must be persisted and resume
  must skip completed steps (replay from the persisted frontier, in the StackStorm and Step
  Functions style). Side-effecting steps make naive re-run dangerous. Needs an explicit model.
- **Still open: the run-view surface.** A workflow run is a tree or DAG of step-runs, so the UI
  (graph, per-step logs and retries, live view) is a large surface. The SSE and log work helps
  per step, but the overall view is unscoped.

Suggested build order, MVP spine first and the clever parts deferred:

1. **Spine.** A linear chain of steps, passing output via a pointer, branch on success or
   error, with checkpoint and resume, and workflow-as-job. This validates the whole model
   cheaply.
2. Conditional branches and multiple branches from a step.
3. Fan-out, with the cap and the collect or short-circuit policy.
4. Shared state, model (a).
5. Loops, with cycle guards and max-iteration bounds.
6. Schema infer, declare, validate, and drift warnings. The wow tier.

## Open questions to pin down later

- Where the workflow definition and run state live (new tables versus reusing `job_runs` per
  step). It must stay distributed in the Store, like everything else.
- Cycle and loop guards (max iterations, timeouts) to keep loops from running away.
- How shared state is scoped, sized, and persisted, and kept secret-safe with no plaintext
  leaks.
- Fan-out concurrency limits and partial-failure semantics when one item fails.
- Pointer language: which existing engine to adopt, with regex as the fallback.
- How "a workflow is also a job" maps onto the scheduler and claim model. A workflow-runner
  kind, perhaps.
- Relationship to the chain-jobs and notifications or webhooks parity items already noted.
