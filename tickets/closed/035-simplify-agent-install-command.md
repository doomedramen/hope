---
id: "035"
title: "Simplify Linux agent installation to one command"
status: closed
priority: high
created: "2026-09-20T22:18:11Z"
updated: "2026-09-21T08:35:15Z"
tags: ["agents", "installation", "enrollment", "ux", "cli"]
---

## Problem

The current Linux agent installation instructions require several manual
steps: creating an enrollment code on the Hope server, transferring it to the
target, downloading the installer, and supplying multiple server URLs and
release arguments. This should be automated away so installation is a single
command run on the host where the agent should be installed.

## Current instructions

These instructions describe the current experience for context only; the
requested work is to replace this multi-step flow with the single-command
flow below.

```text
Enroll a Linux agent
Create a short-lived code, then run the signed installer on the Linux host. The code is single-use and never appears in this UI.
Create an enrollment code
server enroll-token create --ttl-minutes 15
Run this on the Hope server and transfer the printedcode= value to the target host through a trusted channel.
Download the installer
curl -fsSL http://192.168.1.163/install-agent.sh -o /tmp/hope-install-agent.sh && chmod 0700 /tmp/hope-install-agent.sh
Install and enroll the host
sudo /tmp/hope-install-agent.sh \
  --enroll-url https://<hope-host>:8444 \
  --gateway-url wss://<hope-host>:8443 \
  --release-base-url http://192.168.1.163
Replace <hope-host> with the server hostname. The installer prompts for the code without echoing it; add --code-stdin --yes when piping the code from a secret manager.
```

## Requested direction

Provide a single-command installer flow run on the target host, closer to the
simplicity of Beszel's installer:

```text
curl -sL https://get.beszel.dev -o /tmp/install-agent.sh && chmod +x /tmp/install-agent.sh && /tmp/install-agent.sh
```

The requested flow removes the manual server-side token creation, transfer,
and argument wiring from the operator's normal path.

## Resolution

- Added authenticated `POST /api/v1/agent-enrollment`, returning a 15-minute single-use token plus CA fingerprint to the Agents UI.
- Replaced the three-step dialog with one copyable command that streams `/agent/install.sh`, derives listener/release URLs from `HOPE_SERVER`, and passes bootstrap material through the installer environment.
- Kept explicit installer flags and `HSERV`/`HPKEY`/`HHKEY` aliases for recovery and unusual topologies.
- Kept `/install-agent.sh` as a compatibility route and added `/agent/install.sh` as the canonical route.

## Verification

- `pnpm -C apps/web test -- --run src/components/AgentsPage.test.tsx`
- `pnpm -C apps/web build`
- `pnpm -C apps/web lint` (existing warnings only)
- `cargo fmt --check`
- `cargo check -p server`
- `cargo test -p server` (185 passed, 1 ignored)
- `bash -n deploy/agent/install-agent.sh`
- `HOPE_SERVER=hope.example.com HOPE_ENROLLMENT_CODE=token.fingerprint bash deploy/agent/install-agent.sh` reaches the root check without requiring URL flags.
- Live authenticated UI generated a single-use command; phone viewport `390x844` had no horizontal page overflow and kept footer controls visible.
