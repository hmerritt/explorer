#!/usr/bin/env bash
set -euo pipefail

fail() {
  printf 'Deploy: %s\n' "$*" >&2
  exit 1
}

rsync_options=(-rlptz --protect-args --delay-updates --delete-delay --chmod=D755,F644 --itemize-changes)
case "${1:-}" in
  '') [[ $# -eq 0 ]] || fail 'Usage: bash deploy.sh [--dry-run]' ;;
  --dry-run)
    [[ $# -eq 1 ]] || fail 'Usage: bash deploy.sh [--dry-run]'
    rsync_options+=(--dry-run)
    ;;
  *) fail 'Usage: bash deploy.sh [--dry-run]' ;;
esac

website_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd -- "$website_dir"
[[ -f .env.deploy ]] || fail 'Copy .env.deploy.example to .env.deploy and configure it first.'
set -a
source ./.env.deploy
set +a

[[ -n "${SSH_TARGET:-}" ]] || fail 'SSH_TARGET is required.'
[[ "$SSH_TARGET" != -* && "$SSH_TARGET" != *[[:space:]/:]* ]] || fail 'SSH_TARGET must be an SSH alias or user@host.'
[[ -n "${DEPLOY_PATH:-}" && "$DEPLOY_PATH" == /* ]] || fail 'DEPLOY_PATH must be an absolute directory dedicated to this site.'
[[ ! "$DEPLOY_PATH" =~ ^/+$ ]] || fail 'DEPLOY_PATH must not be the remote root directory.'
case "/${DEPLOY_PATH#/}/" in
  */../*|*/./*) fail 'DEPLOY_PATH must not contain . or .. path components.' ;;
esac
[[ -n "${VITE_SITE_URL:-}" ]] || fail 'VITE_SITE_URL is required.'
case "$VITE_SITE_URL" in
  http://?*|https://?*) ;;
  *) fail 'VITE_SITE_URL must be an http:// or https:// public origin.' ;;
esac

for command in bun ssh rsync; do
  command -v "$command" >/dev/null 2>&1 || fail "Required command not found: $command"
done

printf 'Building static website...\n'
bun run build
[[ -s dist/client/index.html ]] || fail 'Build did not produce a nonempty dist/client/index.html.'

# Quote for the remote POSIX shell, including paths containing single quotes.
remote_path=${DEPLOY_PATH//\'/\'\\\'\'}
ssh "$SSH_TARGET" "test -d '$remote_path' && cd '$remote_path' && test -w . && test \"\$(pwd -P)\" != /" \
  || fail 'Remote directory must already exist, be writable and not resolve to /.'

printf 'Syncing static website%s...\n' "${1:+ (dry run)}"
rsync "${rsync_options[@]}" -e ssh -- dist/client/ "${SSH_TARGET}:${DEPLOY_PATH%/}/"
