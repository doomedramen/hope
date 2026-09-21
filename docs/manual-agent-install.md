# Deploying Hope agents

Hope's published image includes a signed Linux `amd64` agent, the
release manifest, and the public trust key. Operators do not create or mount
an `agent-releases` directory. Agents download from Hope itself, not GitHub.

## One command on the target

Open **Agents → Enroll agent**, check the **Agent connection address**, and
copy the generated command. Paste it into a Linux terminal with `sudo`.
The command contains a one-time enrollment code that expires after 15 minutes;
do not share it. It downloads the installer over HTTPS, verifies Hope's TLS
identity, fetches the signed release, installs a systemd service, and checks in.

For a direct LAN install, open Hope at `http://<hope-ip>` and use that HTTP
origin as the connection address. Hope exposes the UI on port 80 and pinned
agent HTTPS on port 443. The generated command includes the TLS public-key
pin; the agent keeps Hope's leaf-certificate fingerprint for later connections. The local
HTTP UI is intended only for a trusted LAN.

If Hope is behind Nginx Proxy Manager, make one HTTPS Proxy Host pointing to
Hope's HTTP port 80 with **WebSocket support enabled**. Open Hope through the
public HTTPS host and use that origin as the connection address. Agent TLS
uses the proxy's normal publicly trusted certificate. No NPM Stream, extra
agent port, or Hope-specific certificate name is needed. After an NPM restart,
the agent reconnects through the same address.

## Deploy from a device

Open **Infrastructure overview**, select a device, and choose **Deploy agent**.
Enter the target's SSH address, port, username, and password or private key.
You can select an existing credential or explicitly save the new one. Hope
pauses at an unfamiliar or changed SSH host key: compare the displayed
fingerprint with the host before trusting it. The dialog shows the deployment
job and waits for the first agent check-in. If the credential was not saved,
Hope deletes it after the completed job. Hope must be able to reach the
target's SSH port; the agent needs only outbound HTTPS/WebSocket access to
Hope's connection address.

## Troubleshooting

- A 503 from `/agent/v1/releases/latest/linux/amd64` means the
  running image does not contain a verified matching release. Use an official
  published Hope image; production publishing is blocked without signing.
- If the installer cannot reach Hope, edit the connection address to an IP or
  hostname resolvable **from the target**, then generate a fresh command.
- If SSH deployment pauses, review the host-key fingerprint in the same
  dialog. Never trust an unexpected changed key without checking the host.
- Enrollment codes are one-use and expire after 15 minutes; generate another
  command if one has expired.
