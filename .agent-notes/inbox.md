## 2026-10-16 — Gaps found while chasing 250 phantom hosts on VLAN 30

- **No way to forget a host.** There is no delete-host API or UI (single or bulk). Stale hosts never expire, so cleaning up after a network fault needed hand-written SQL on the server.
- **One machine at many addresses shows as many devices.** A Debian 13 LXC (prowlarr) with a `dhclient` that never dropped old leases held ~247 addresses; HOPE listed each as a separate device. Idea: treat addresses sharing an SSH host key as one host (keys are already readable via `ssh::fingerprint`), and flag a key seen across a whole range.
- **Deleting a network drops its scan history but keeps its hosts** (`api::delete_network`), which made "when was this range last scanned" hard to answer during diagnosis.
- **Migration 5 (`forget_refusal_only_hosts`) matched nothing in this case** — the phantoms were real ICMP/TCP answers, not refusals. The TCP-refusal fix (60450a6) is still correct for firewalls that reset routed traffic.
- **Active ARP stopped after the HOPE LXC rebooted:** "no IPv4 Ethernet interface could be opened for raw packets". Unexplained; worth checking the container's capabilities.
