#!/usr/bin/env bash
set -u

CONTAINER_NAME="${CONTAINER_NAME:-andromeda-galaxy}"
DB_PATH="${DB_PATH:-/var/lib/docker/volumes/andromeda-galaxy_andromeda-data/_data/andromeda.db}"
CHECK_INTERVAL_SECONDS="${CHECK_INTERVAL_SECONDS:-60}"
MAX_SILENCE_SECONDS="${MAX_SILENCE_SECONDS:-900}"
RESTART_COOLDOWN_SECONDS="${RESTART_COOLDOWN_SECONDS:-600}"
POST_RESTART_GRACE_SECONDS="${POST_RESTART_GRACE_SECONDS:-180}"
OK_LOG_EVERY_SECONDS="${OK_LOG_EVERY_SECONDS:-300}"
STATE_DIR="${STATE_DIR:-/var/lib/andromeda-watchdog}"
STATE_FILE="${STATE_FILE:-${STATE_DIR}/last_restart_epoch}"

last_ok_log=0

log() {
  printf '%s [ANDROMEDA_WATCHDOG] %s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$*"
}

ensure_state_dir() {
  mkdir -p "$STATE_DIR" 2>/dev/null || true
}

command_exists() {
  command -v "$1" >/dev/null 2>&1
}

last_restart_epoch() {
  if [ -f "$STATE_FILE" ]; then
    cat "$STATE_FILE" 2>/dev/null || printf '0'
  else
    printf '0'
  fi
}

mark_restart() {
  ensure_state_dir
  date +%s >"$STATE_FILE" 2>/dev/null || true
}

restart_container() {
  reason="$1"
  now="$(date +%s)"
  last_restart="$(last_restart_epoch)"
  restart_age=$((now - last_restart))

  if [ "$last_restart" -gt 0 ] && [ "$restart_age" -lt "$RESTART_COOLDOWN_SECONDS" ]; then
    log "restart skipped: reason=${reason}; cooldown_remaining=$((RESTART_COOLDOWN_SECONDS - restart_age))s"
    return 0
  fi

  log "restarting container: name=${CONTAINER_NAME}; reason=${reason}"
  if docker restart "$CONTAINER_NAME" >/dev/null 2>&1; then
    mark_restart
    log "restart OK; waiting grace=${POST_RESTART_GRACE_SECONDS}s"
    sleep "$POST_RESTART_GRACE_SECONDS"
  else
    log "restart FAILED; container=${CONTAINER_NAME}"
  fi
}

read_container_state() {
  docker inspect \
    --format '{{.State.Status}} {{if .State.Health}}{{.State.Health.Status}}{{else}}no-healthcheck{{end}}' \
    "$CONTAINER_NAME" 2>/dev/null || true
}

read_last_sent_epoch() {
  sqlite3 "$DB_PATH" 'select coalesce(max(sent_at), 0) from sent_messages;' 2>/dev/null || printf '0'
}

check_once() {
  if ! command_exists docker; then
    log "docker command not found"
    return 1
  fi

  if ! command_exists sqlite3; then
    log "sqlite3 command not found"
    return 1
  fi

  container_state="$(read_container_state)"
  if [ -z "$container_state" ]; then
    log "container not found: name=${CONTAINER_NAME}"
    return 1
  fi

  status="$(printf '%s' "$container_state" | awk '{print $1}')"
  health="$(printf '%s' "$container_state" | awk '{print $2}')"

  if [ "$status" != "running" ]; then
    restart_container "container_status_${status}"
    return 0
  fi

  if [ "$health" = "unhealthy" ]; then
    restart_container "container_unhealthy"
    return 0
  fi

  if [ ! -f "$DB_PATH" ]; then
    log "database not found: path=${DB_PATH}"
    return 1
  fi

  last_sent="$(read_last_sent_epoch)"
  case "$last_sent" in
    ''|*[!0-9]*)
      log "invalid last_sent value: ${last_sent}"
      return 1
      ;;
  esac

  if [ "$last_sent" -le 0 ]; then
    restart_container "no_telegram_messages_recorded"
    return 0
  fi

  now="$(date +%s)"
  silence_seconds=$((now - last_sent))

  if [ "$silence_seconds" -gt "$MAX_SILENCE_SECONDS" ]; then
    restart_container "telegram_silence_${silence_seconds}s"
    return 0
  fi

  if [ $((now - last_ok_log)) -ge "$OK_LOG_EVERY_SECONDS" ]; then
    log "OK: status=${status}; health=${health}; silence=${silence_seconds}s; max=${MAX_SILENCE_SECONDS}s"
    last_ok_log="$now"
  fi
}

log "started: container=${CONTAINER_NAME}; max_silence=${MAX_SILENCE_SECONDS}s; interval=${CHECK_INTERVAL_SECONDS}s"

while true; do
  check_once
  sleep "$CHECK_INTERVAL_SECONDS"
done
