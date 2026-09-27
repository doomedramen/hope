# Facilitator setup and observation guide

This file is for the study owner. Do not give it to participants before their session.

## Recruit and assign sessions

Use five real people with a mix of Linux and Hope experience. Use IDs P01–P05 rather than names in shared evidence. Include at least one 375-pixel-wide mobile session and one keyboard-only session; ensure terminal access is available for enrollment. If a mobile participant cannot access the test terminal, have them complete the web setup steps and mark terminal installation separately as assisted or not evaluated. Do not count it as an unaided enrollment success.

The participant pack is self-guided, but preparing an honest failure scenario still requires a study owner. Complete and verify every fixture before inviting someone. A broken fixture is an environment failure, not a usability failure.

## Prepare isolated fixtures

Use a separate Hope deployment and disposable Linux targets. Keep preview and production targets out of scope. Follow [Agent data and updates](../../agent-data-and-updates.md) and the native checks in [Operational validation](../../agent-operations-validation.md). Do not relax signature verification to prepare the study.

| Task | Fixture required | Owner verification before session |
| --- | --- | --- |
| 1 | Fresh disposable Linux host with the supported installer prerequisites, plus a separate existing agent | A new attempt can enroll; the existing agent cannot complete another attempt's milestones. Give temporary terminal access separately. |
| 2 | A named host with old readings and a known reason, such as a disconnected disposable target | Record the last good sample/contact and true cause privately. Verify the first view shows stale/unavailable values honestly. Do not explain the cause in the session sheet. |
| 3 | A recorded resource spike with matching, non-sensitive source events in a known time range | Verify the graph contains the spike and the logs exist. Record exact timestamps and expected evidence privately. Include continuing harmless log traffic to exercise reading stability. |
| 4 | A selected journal test source generating both info and warning entries | Capture initial policy/revision. Be ready to emit a controlled 20-info/20-warning batch after the participant's revision is applied. Ensure this source's current one-minute budget has reset. |
| 5 | A disposable agent with a valid previous version and an approved, correctly signed test release that fails health verification | Rehearse supervised rollback before the session. Confirm the previous release and continued telemetry are observable. Never offer this test artifact to production agents. Use the existing safe rollback fixture or have the release owner prepare it. |

For each scenario, record privately: agent identity, source, timestamps, actual connection state, expected version/revision, and expected result. Supply participants only the neutral host/source/time labels from the session sheet. If a rollback fixture is unavailable, mark task 5 “not evaluated”; do not substitute a fictional result or a facilitator demonstration and call it unaided success.

Before starting, test access, viewport, keyboard focus, background workers, selected log permissions, artifact availability, and the clock/time zone. Preserve a disposable-environment snapshot or reset procedure. Reset settings, budgets, update outcomes and new enrollment attempts between participants so every person gets a comparable task.

## Observe without teaching navigation

Ask for the participant's first-view impression before they interact. Then let them work. Record wrong destinations and their exact wording, not your interpretation of what they “must have meant.” Give help only when requested or needed to avoid an out-of-scope action. Any help changes the task's unaided result to “no.” Waiting for the system is separate from active searching time.

Success criteria for the owner:

1. The participant identifies the newly enrolled host and uses received data/milestones to establish readiness. Optional logs are not confused with failed basic enrollment.
2. They distinguish connection state from stale collection or queued delivery and support the explanation with visible evidence.
3. They locate the spike, investigate the corresponding log period, and read relevant events without losing their place. “No causal evidence found” is valid when supported by the fixture.
4. They select warning-or-higher and five records per minute, confirm the acknowledged revision, and recognize the difference between severity filtering and quota gaps. The controlled batch should yield five warnings, zero info entries and a gap diagnostic accounting for 15 warnings.
5. They distinguish queued/in-progress/failed/rolled-back states, identify the restored version and check that data collection resumed. Do not end observation at a successful request submission.

## Review and retest

Record “not evaluated” for missing fixtures and “stopped” for abandoned attempts. Do not count either as a pass. Review repeated first-view or state-interpretation failures before cosmetic comments. Treat a single dangerous misunderstanding, such as interpreting a queued update as completed, as worth investigation even if it is not repeated. Link fixes to the evidence row and repeat affected tasks after corrections. Five sessions are formative evidence, not a statistically representative success rate.
