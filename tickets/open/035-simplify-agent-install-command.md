---
id: "035"
title: "Simplify Linux agent installation to one command"
status: open
priority: high
created: "2026-09-20T22:18:11Z"
updated: "2026-09-20T22:18:11Z"
tags: ["agents", "installation", "enrollment", "ux", "cli"]
---

## Problem

The current Linux agent installation instructions require several manual
steps: creating an enrollment code on the Hope server, transferring it to the
target, downloading the installer, and supplying multiple server URLs and
release arguments. This should be automated away so installation is a single
command run on the host where the agent should be installed.

## Current instructions

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
