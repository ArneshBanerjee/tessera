#!/bin/sh
# Starts a throwaway Tessera server and runs the compatibility suites against it.
# Usage: scripts/compat/run.sh
set -eu
cd "$(dirname "$0")/../.."
dir=$(mktemp -d)
trap 'kill "$server" 2>/dev/null || true; rm -rf "$dir"' EXIT
db="sqlite://$dir/compat.db"
port=8099
cargo build -q -p tessera-server
./target/debug/tessera --database "$db" create-user --email admin@compat.test --name Admin --password pw --admin >/dev/null
token=$(./target/debug/tessera --database "$db" create-token --email admin@compat.test)
./target/debug/tessera --database "$db" serve --addr "127.0.0.1:$port" >"$dir/server.log" 2>&1 &
server=$!
for _ in $(seq 1 50); do curl -sf "http://127.0.0.1:$port/healthz" >/dev/null && break; sleep 0.2; done
uv run --quiet scripts/compat/python_jira.py "http://127.0.0.1:$port" admin@compat.test "$token"
uv run --quiet scripts/compat/python_confluence.py "http://127.0.0.1:$port" admin@compat.test "$token"
