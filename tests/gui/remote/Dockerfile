# Test remote for gilvt's SSH acceptance cases (tests/gui/remote.sh).
FROM debian:bookworm-slim
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends openssh-server tmux zsh fish bash git procps ca-certificates >/dev/null \
 && rm -rf /var/lib/apt/lists/* && mkdir /run/sshd \
 && useradd -m -s /bin/bash dev && useradd -m -s /usr/bin/zsh devz \
 && printf 'PasswordAuthentication no\nKbdInteractiveAuthentication no\nAllowTcpForwarding yes\nPermitUserEnvironment no\n' >> /etc/ssh/sshd_config
COPY authorized_keys /tmp/authorized_keys
RUN for u in dev devz; do install -d -m 700 -o $u -g $u /home/$u/.ssh && install -m 600 -o $u -g $u /tmp/authorized_keys /home/$u/.ssh/authorized_keys; done \
 && touch /home/devz/.zshrc && chown devz:devz /home/devz/.zshrc
CMD ["/usr/sbin/sshd", "-D", "-e"]
