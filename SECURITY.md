# Security policy

## Reporting a vulnerability

Do not open a public issue for an unpatched vulnerability. Send a report to
`wookoouk@gmail.com` with subject `[hope security]`, or use GitHub's private
security-advisory flow for `doomedramen/hope` when it is enabled.

Include:

- affected commit, release, or deployment image;
- component and configuration;
- impact and attack prerequisites;
- reproducible steps or a minimal proof of concept; and
- any known mitigation or disclosure deadline.

Encrypt sensitive attachments with a key supplied by the maintainer before
sending them. Do not include real passwords, credential master keys, CA keys,
agent private keys, signing keys, enrollment tokens, or production dumps in a
report.

The maintainer will acknowledge a report when practical, investigate scope and
severity, coordinate a fix or mitigation, and credit reporters who want credit.
There is no guaranteed response or disclosure timeline for this early project.

## Deployment security

The Compose example is not a complete perimeter. Keep the main HTTP listener
behind TLS, keep `HOPE_COOKIE_SECURE=true` there, firewall the 8443/8444
listeners, protect PostgreSQL, and back up the credential master key and
server PKI separately from the database. Read
[`docs/threat-model.md`](docs/threat-model.md) before exposing the service.
