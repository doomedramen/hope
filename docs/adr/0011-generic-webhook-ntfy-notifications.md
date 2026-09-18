# 0011. Generic webhook + ntfy for notifications

## Status
Accepted

## Date
2026-09-18

## Context
Spec §8.5 requires the first release to support a generic outbound webhook plus at least one of ntfy, Gotify, Discord, email, or Pushover, along with recovery notifications, maintenance notifications, delays/severities, a test-notification function, provider health/errors, and quiet hours.

## Decision
Ship two notification providers for v1: a generic outbound webhook (satisfying any integration not built in directly) and ntfy (a simple, self-hostable push provider that fits the homelab-first posture of the project).

## Alternatives considered
- **Discord** — deferred. Popular, but tied to a third-party hosted service and a bot/webhook setup flow that adds little over the generic webhook for users who already use Discord (they can point the generic webhook at Discord's own webhook URL).
- **Email (SMTP)** — deferred. Useful but requires SMTP configuration/credential handling that duplicates general credential-type work (§12.1) without adding new notification-routing capability beyond what webhook + ntfy already cover for v1.
- **Gotify** — deferred in favour of ntfy; both are self-hosted push options, and ntfy's simpler HTTP POST model and hosted-or-self-hosted flexibility make it the better first pick. Gotify remains a natural second provider post-v1.

## Consequences
The notification routing engine (severity, delay, quiet hours, maintenance suppression — §8.5) is built provider-agnostic from the start, so adding Discord/email/Gotify/Pushover later is a matter of implementing another provider, not redesigning routing. Users who want Discord/email today can reach them via the generic webhook as an interim path.
