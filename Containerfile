# Runtime image for the bot. Same base and hardening conventions as ~/projects/AI_Chatbot.
FROM docker.io/library/archlinux@sha256:bf88f8143c3ef21b14d0ebb8140099f0bf75ecfc3699f357ff92317a73f5aba9 AS builder

RUN pacman -Syu --noconfirm --needed ca-certificates cargo rust gcc \
    && pacman -Scc --noconfirm
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked -p mev-bot \
    && install -Dm0755 target/release/mev-bot /out/mev-bot \
    && strip /out/mev-bot

FROM docker.io/library/archlinux@sha256:bf88f8143c3ef21b14d0ebb8140099f0bf75ecfc3699f357ff92317a73f5aba9
RUN for attempt in 1 2 3; do \
      pacman -Syu --noconfirm --needed ca-certificates && installed=1 && break; \
      sleep 5; \
    done \
    && test "${installed:-0}" = 1 \
    && update-ca-trust \
    && pacman -Scc --noconfirm
COPY --from=builder /out/mev-bot /usr/local/bin/mev-bot
COPY config/ /app/config/
# The only writable state: event logs, heartbeat, and the STOP kill switch.
# Ids are build args because compose runs this as the host user under userns keep-id.
ARG APP_UID=1000
ARG APP_GID=1000
RUN install -d -o ${APP_UID} -g ${APP_GID} -m 0700 /var/lib/mev-bot
USER ${APP_UID}:${APP_GID}
WORKDIR /app
ENTRYPOINT ["/usr/local/bin/mev-bot", "--data-dir", "/var/lib/mev-bot"]
CMD ["--mode", "shadow"]
