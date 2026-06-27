# Notifications, webhooks, triggers, sensors — brainstorm notes

> Status: **BRAINSTORM, not a spec.** Rough notes from a design discussion, parked for later.
> Pairs with [WORKFLOWS.md](WORKFLOWS.md) (the trigger and event-driven sides converge there).

## What turned out to be redundant

- **Outbound webhooks are not a feature.** Sending an HTTP POST is already the HTTP runner
  (with retry, backoff, status classification). The only thing missing is the **trigger**:
  jobs are scheduled, with no way to say "do this when X happens." So the real primitive is an
  **event trigger**, and an outbound webhook is then a thin channel on top, not its own thing.
- **Inbound webhooks are a trigger source.** An external system hitting an arbiter URL to
  start something is useful, but it is a trigger (a sibling of cron), and the request-body-
  flows-into-steps part wants the workflow data-flow machinery. Defer to the workflow/trigger
  layer.

## The actually-distinct feature: alerting humans

Tell the people responsible for a tenant that something needs attention. Not just an HTTP call,
because the value is the **policy layer** (severity, grouping/dedup, rate-limit, routing,
muting), which is what every "just fire a webhook" setup reinvents badly.

- **Events worth raising:** run failed after retries, job flapping (N consecutive failures),
  run ran too long or missed an SLA, worker offline, schedule misfired, plus admin-only ones
  (KEK or secret issues, node eviction).
- **Who gets them:** tenant-scoped events go to that tenant's operators and admins, cluster
  events to system admins. Per-user preferences (which events, which channels, mute or snooze,
  immediate versus digest).
- **Channels:** start with an in-app inbox (a bell or feed, zero external deps, immediately
  useful, dogfoods the event layer), then email, Slack or Discord, and a generic webhook.

## The reframe: one event layer, three consumers

Build the event layer once (emit typed events at finalize, the reaper, the scheduler). Then,
in increasing cost:

1. **In-app alert inbox** (cheap, no deps) — build first.
2. **External alert channels** (email, Slack, webhook) — reuse the HTTP, retry, and secrets
   machinery.
3. **Event-triggered actions** (outbound webhook, "trigger a job or workflow on event") —
   where notifications, inbound triggers, and workflows all converge.

### Distributed delivery notes (for when external channels get built)

- **Transactional outbox.** Enqueue delivery rows in the same DB as the state change so a crash
  cannot lose the intent.
- **Claim to dispatch.** Deliveries are claimed `FOR UPDATE SKIP LOCKED` like job runs, so
  exactly one node sends each one. Retry with backoff (reuse the run retry primitives),
  dead-letter after max attempts.
- **At-least-once.** Each payload carries an idempotency key the receiver dedupes on, plus an
  HMAC signature header so the receiver can verify authenticity. Webhook auth is a
  `secret:<name>` ref.
- Likely a dedicated `notification_rules` + `notification_deliveries` pair rather than
  modeling deliveries as job runs, since they fan out per event and need idempotency and
  signing, but the claim and backoff code transfers.

## Competitive gaps (after webhooks + workflows land)

With event triggers and workflows, arbiter is architecturally ahead of Cronicle and
StackStorm. What they still have splits into "time and ecosystem" versus "real capability."

Real capability gaps worth considering, in rough value order:

1. **Sensor / trigger-source framework** (StackStorm). React to the outside world, not just
   cron and inbound webhook: poll an API, watch a queue or file, tail a stream, and emit
   triggers. This is the top of the event/trigger layer and the thing that makes it an
   automation platform rather than a scheduler.
2. **Performance and resource metrics + limits per run** (Cronicle). Track and cap CPU and
   memory per run, with duration and trend charts, not just a concurrency count.
3. **Human-in-the-loop approval steps** in workflows (StackStorm inquiries). The KEK approval
   gate is a small taste of this.
4. **Finer-grained RBAC** as multi-team usage grows (we have tenancy plus three roles).

Time and ecosystem, not worth chasing directly: integration-pack ecosystems and registries,
ChatOps, community, and docs maturity.
