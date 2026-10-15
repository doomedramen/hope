# Disposable Linux installer target: Debian 13 with systemd as PID 1 and a real OpenSSH server.
# Build per platform: docker build --platform linux/amd64 -f tests/gates/target.Dockerfile -t hopegate-target:amd64 tests/gates
FROM debian:trixie
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends systemd systemd-sysv dbus libpam-systemd openssh-server openssh-client sudo curl ca-certificates openssl procps iproute2 less \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /run/sshd \
    && systemctl enable ssh \
    && systemctl mask getty.target console-getty.service systemd-logind-keep-nothing.service 2>/dev/null; true
# Three accounts exercise the installer modes: root, an administrator with passwordless sudo, and a plain user.
RUN useradd -m -s /bin/bash hopesudo && useradd -m -s /bin/bash hopeplain \
    && echo 'hopesudo ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/hopesudo && chmod 440 /etc/sudoers.d/hopesudo \
    && printf 'PermitRootLogin yes\nPasswordAuthentication yes\nKbdInteractiveAuthentication no\n' > /etc/ssh/sshd_config.d/10-gates.conf
# Emulation workaround, amd64 only: Rosetta (OrbStack) raises SIGTRAP in systemd units that set
# MemoryDenyWriteExecute=yes (journald, logind, ...). Switch it off for those shipped units so systemd
# itself runs; the agent's own unit is untouched. Native arm64 needs nothing.
ARG TARGETARCH
RUN if [ "$TARGETARCH" = amd64 ]; then \
        for f in $(grep -l '^MemoryDenyWriteExecute=yes' /usr/lib/systemd/system/*.service); do \
            u=$(basename "$f"); mkdir -p "/etc/systemd/system/$u.d"; \
            printf '[Service]\nMemoryDenyWriteExecute=no\n' > "/etc/systemd/system/$u.d/10-rosetta.conf"; \
        done; \
    fi
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
