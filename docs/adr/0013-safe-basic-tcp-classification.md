# 0013. Bounded observation-only TCP protocol classification

## Context

Milestone 2 needs enough protocol information to make every open TCP port
useful in the canonical inventory. The scan must remain safe on fragile
homelab devices and must not turn discovery into authentication, product
fingerprinting, or an unbounded web crawler.

TLS creates a separate trust concern. A discovery worker needs to recognize
TLS and retain certificate identity metadata even when a device uses a
self-signed or privately issued certificate. Treating that handshake as proof
of trust would make an inventory probe a credential-bearing client.

## Decision

Use a bounded classifier for resolved open TCP observations. It performs a
passive bounded read for banners, one ordinary HTTP `GET` without credentials
or cookies, and one TLS handshake followed by the same safe request when
appropriate. Connects, reads, header/body sizes, redirect hops, and total
connections are bounded. Redirects stay on the scanned address and port.

The TLS client verifies handshake signatures but uses an observation-only
certificate verifier. It records certificate fingerprint, subject/SAN, issuer,
validity, and ALPN while explicitly marking certificate trust as
`not_validated`. It never supplies client certificates, credentials, cookies,
or authorization headers.

Classification writes `network_scan` evidence on the existing canonical M1
service and reuses one `socket` endpoint per device/address/port. Unknown
open ports use protocol `tcp`, so failed or unrecognized probes remain
monitorable. Repeated identical classifications do not create duplicate
services or change events. Confirmed manual protocol evidence outranks
automatic classification.

## Consequences

M2 can show generic TCP, HTTP, HTTPS/TLS, and SSH services before M3 product
signatures exist. Certificate metadata remains useful for review without
being mistaken for a trust decision. A custom TLS service may need a later
M3 protocol rule for richer identification, and host-key collection remains
outside the SSH banner probe.

## Alternatives rejected

- **Use a general HTTP client with automatic redirects and credential support**
  — rejected because request scope, body size, redirect destinations, and
  sensitive headers would be harder to audit.
- **Require normal public-CA certificate validation** — rejected because
  homelab services commonly use private or self-signed certificates and M2
  only needs safe classification, not authenticated communication.
- **Create a new fingerprint/service table** — rejected because M1's service,
  endpoint, and evidence tables are the canonical model required by the
  specification.
