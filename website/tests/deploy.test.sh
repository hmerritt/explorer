#!/usr/bin/env bash
set -euo pipefail

website_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
bash -n "$website_dir/deploy.sh"
fixture="$(mktemp -d)"
trap 'rm -rf -- "$fixture"' EXIT
mkdir -p "$fixture/site" "$fixture/bin" "$fixture/log" "$fixture/Explorer's site"
cp "$website_dir/deploy.sh" "$fixture/site/deploy.sh"
export TEST_LOG="$fixture/log"
export PATH="$fixture/bin:$PATH"
unset SSH_TARGET DEPLOY_PATH VITE_SITE_URL

cat > "$fixture/bin/bun" <<'SH'
#!/usr/bin/env bash
set -eu
printf '%s\n' "$@" > "$TEST_LOG/bun"
[[ "$*" == 'run build' ]]
[[ "$VITE_SITE_URL" == 'https://explorer.example.com' ]]
[[ "${STUB_BUILD_FAIL:-0}" == 0 ]] || exit 7
mkdir -p dist/client
[[ "${STUB_EMPTY_BUILD:-0}" == 0 ]] || exit 0
printf '<html>Built site</html>\n' > dist/client/index.html
SH
cat > "$fixture/bin/ssh" <<'SH'
#!/usr/bin/env bash
set -eu
printf '%s\n' "$@" > "$TEST_LOG/ssh"
[[ "${STUB_SSH_FAIL:-0}" == 0 ]] || exit 9
sh -c "$2"
SH
cat > "$fixture/bin/rsync" <<'SH'
#!/usr/bin/env bash
set -eu
printf '%s\n' "$@" > "$TEST_LOG/rsync"
[[ "${STUB_RSYNC_FAIL:-0}" == 0 ]] || exit 11
SH
chmod +x "$fixture/bin/"*

configure() {
  printf 'SSH_TARGET=%q\nDEPLOY_PATH=%q\nVITE_SITE_URL=%q\n' \
    "${2:-explorer-vps}" "$1" 'https://explorer.example.com' > "$fixture/site/.env.deploy"
}

expect_failure() {
  if bash "$fixture/site/deploy.sh" "$@" > "$fixture/output" 2>&1; then
    printf 'Unexpected success: %s\n' "$*" >&2
    exit 1
  fi
  [[ ! -f "$TEST_LOG/rsync" ]] || { printf 'Unsafe transfer attempted\n' >&2; exit 1; }
}

# Missing config and unsafe paths stop before building or contacting the VPS.
expect_failure
for path in '' / // /. /./ /srv/../var/www relative/path; do
  configure "$path"
  expect_failure
  [[ ! -f "$TEST_LOG/bun" && ! -f "$TEST_LOG/ssh" ]]
done
configure "$fixture/Explorer's site" '-invalid-host'
expect_failure
configure "$fixture/Explorer's site"
printf "VITE_SITE_URL=''\n" >> "$fixture/site/.env.deploy"
expect_failure
configure "$fixture/Explorer's site"
printf "VITE_SITE_URL='not-a-url'\n" >> "$fixture/site/.env.deploy"
expect_failure
configure "$fixture/Explorer's site"
expect_failure --unknown
expect_failure --dry-run extra

# A build error must not upload stale output from an earlier build.
mkdir -p "$fixture/site/dist/client"
printf 'Old site\n' > "$fixture/site/dist/client/index.html"
export STUB_BUILD_FAIL=1
expect_failure
[[ ! -f "$TEST_LOG/ssh" ]]
unset STUB_BUILD_FAIL
rm -- "$fixture/site/dist/client/index.html"
export STUB_EMPTY_BUILD=1
expect_failure
[[ ! -f "$TEST_LOG/ssh" ]]
unset STUB_EMPTY_BUILD

# The remote directory must exist and be writable; a symlink to / is rejected.
configure "$fixture/nonexistent"
expect_failure
ln -s / "$fixture/root-link"
configure "$fixture/root-link"
expect_failure
configure "$fixture/Explorer's site"
export STUB_SSH_FAIL=1
expect_failure
unset STUB_SSH_FAIL

# Running from another directory still finds the local configuration and build.
configure "$fixture/Explorer's site/"
(cd / && bash "$fixture/site/deploy.sh" --dry-run) > "$fixture/output" 2>&1
cat > "$fixture/expected" <<EOF
-rlptz
--protect-args
--delay-updates
--delete-delay
--chmod=D755,F644
--itemize-changes
--dry-run
-e
ssh
--
dist/client/
explorer-vps:$fixture/Explorer's site/
EOF
diff -u "$fixture/expected" "$TEST_LOG/rsync"
[[ "$(head -n 1 "$TEST_LOG/ssh")" == explorer-vps ]]

# Real mode has the same mirror settings, without --dry-run.
bash "$fixture/site/deploy.sh" > "$fixture/output" 2>&1
if grep -qx -- '--dry-run' "$TEST_LOG/rsync"; then exit 1; fi
export STUB_RSYNC_FAIL=1
if bash "$fixture/site/deploy.sh" > "$fixture/output" 2>&1; then exit 1; fi
unset STUB_RSYNC_FAIL
printf 'Deployment script checks passed. No VPS was contacted.\n'
