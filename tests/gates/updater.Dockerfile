# Disposable Linux target for the updater gate (tests/gates/updater.py): Debian 13 with systemd as PID 1,
# plus the tools the agent installer requires. The base image is maintained for running systemd in a
# container; the gate runs it with --privileged --cgroupns=host and a writable /sys/fs/cgroup.
# HOPEGATE_UPDATER_IMAGE can name a prebuilt image instead (systemd as PID 1, curl, openssl).
FROM jrei/systemd-debian:13
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates openssl procps util-linux \
    && rm -rf /var/lib/apt/lists/*
