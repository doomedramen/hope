FROM debian:trixie-slim

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        openssh-server \
        sudo \
        systemd \
        systemd-sysv \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && echo 'root:hope-e2e-root' | chpasswd \
    && mkdir -p /run/sshd \
    && printf '%s\n' \
        'PermitRootLogin yes' \
        'PasswordAuthentication yes' \
        'UsePAM no' \
        > /etc/ssh/sshd_config.d/hope-e2e.conf \
    && ln -sf /lib/systemd/system/ssh.service /etc/systemd/system/multi-user.target.wants/ssh.service

STOPSIGNAL SIGRTMIN+3

CMD ["/sbin/init"]
