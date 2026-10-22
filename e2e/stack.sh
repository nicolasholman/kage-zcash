#!/usr/bin/env bash
# Starts the real Kage stack for the browser E2E run, bound to 127.0.0.1. Each x402z-server has a
# public port (paid API only) and an admin port (dashboard, feed, status, plus the paid API):
#   4021        x402z-facilitator  (published demo UIVK only, a throwaway receipt key made here)
#   4020 / 4120 x402z-server       (merchant view, the default)
#   4030 / 4130 x402z-server       (facilitator URL points at a closed port: the "unreachable" state)
#   4040 / 4140 x402z-server       (X402Z_DEMO_AGENT_LEDGER = a ledger the real agent wrote here)
#   4041 / 4141 x402z-server       (X402Z_DEMO_AGENT_LEDGER = a missing file: the Setup required state)
#   4042 / 4142 x402z-server       (X402Z_CHALLENGE_RATE_PER_MIN=1: the 429 state)
#   4050        landing/           (served under /zec-shielded-x402/, like GitHub Pages)
# No seed and no spending key is used anywhere. PIDs go to e2e/.run/pids; `stack.sh stop` kills them.
set -euo pipefail
cd "$(dirname "$0")/.."
RUN=e2e/.run
BIN=target/debug

if [[ "${1:-}" == "stop" ]]; then
  [[ -f $RUN/pids ]] || { echo "no $RUN/pids"; exit 0; }
  while read -r name pid; do kill "$pid" 2>/dev/null && echo "killed $name ($pid)" || echo "$name ($pid) already gone"; done < "$RUN/pids"
  rm -f "$RUN/pids"
  exit 0
fi

for b in x402z-server x402z-facilitator x402z-agent examples/check_uivk; do
  [[ -x $BIN/$b ]] || { echo "error: $BIN/$b missing; run: cargo build -p x402z-server -p x402z-facilitator -p x402z-agent --bins --examples" >&2; exit 1; }
done
[[ -f $RUN/pids ]] && { echo "error: $RUN/pids exists; run e2e/stack.sh stop first" >&2; exit 1; }

rm -rf "$RUN"; mkdir -p "$RUN/wallet"; chmod 700 "$RUN"
# The demo merchant's UIVK is published in the README on purpose (testnet, incoming only).
UIVK=$(grep -o 'uivktest1[a-z0-9]\{100,\}' README.md | head -1)
UA=$($BIN/examples/check_uivk address "$UIVK")
# A fresh receipt key for this run only; the secret stays in a 0600 file and is never printed.
( umask 077; $BIN/x402z-facilitator keygen > "$RUN/receipt.env" )
PUB=$(sed -n 's/^X402Z_RECEIPT_PUBKEY=//p' "$RUN/receipt.env")

start() { # name logfile cmd...
  local name=$1; shift
  "$@" > "$RUN/$name.log" 2>&1 &
  echo "$name $!" >> "$RUN/pids"
}

start facilitator env X402Z_MERCHANT_UIVK="$UIVK" X402Z_FACILITATOR_BIND=127.0.0.1:4021 \
  X402Z_FACILITATOR_DB="$RUN/facilitator.sqlite" \
  bash -c 'set -a; . "$0"; set +a; exec "$1"' "$RUN/receipt.env" "$BIN/x402z-facilitator"

server() { # name public-port admin-port extra-env...
  local name=$1 port=$2 admin=$3; shift 3
  start "$name" env X402Z_MERCHANT_UA="$UA" X402Z_RECEIPT_PUBKEY="$PUB" X402Z_FACILITATOR_URL=http://127.0.0.1:4021 \
    X402Z_BIND=127.0.0.1:$port X402Z_ADMIN_BIND=127.0.0.1:$admin X402Z_SERVER_DB="$RUN/$name.sqlite" \
    X402Z_CHALLENGE_RATE_PER_MIN=200 "$@" "$BIN/x402z-server"
}
server server 4020 4120
server server-nofac 4030 4130 X402Z_FACILITATOR_URL=http://127.0.0.1:4039
server server-ledger 4040 4140 X402Z_DEMO_AGENT_LEDGER="$PWD/$RUN/agent-ledger.json"
server server-badledger 4041 4141 X402Z_DEMO_AGENT_LEDGER="$PWD/$RUN/no-such-ledger.json"
server server-ratelimit 4042 4142 X402Z_CHALLENGE_RATE_PER_MIN=1
start landing node e2e/serve-landing.mjs 4050

wait_http() { for _ in $(seq 1 100); do curl -fs -o /dev/null "$1" && return 0; sleep 0.3; done; echo "error: $1 never came up" >&2; return 1; }
for p in 4120 4130 4140 4141 4142; do wait_http "http://127.0.0.1:$p/api/status"; done
wait_http http://127.0.0.1:4050/zec-shielded-x402/
for _ in $(seq 1 100); do grep -q "listening" "$RUN/facilitator.log" 2>/dev/null && break; sleep 0.3; done

# Real spendguard decisions for the ledger panel: the agent asks the 4040 server for a 402, and
# spendguard refuses before any wallet is opened (no seed is set, nothing is built or broadcast).
agent() { env X402Z_TARGET_URL=http://127.0.0.1:4040/api/quote X402Z_AGENT_LEDGER="$RUN/agent-ledger.json" \
  X402Z_AGENT_WALLET_DIR="$RUN/wallet" X402Z_AGENT_NAME=e2e-agent "$@" "$BIN/x402z-agent" >> "$RUN/agent.log" 2>&1 || echo "agent exit $?" >> "$RUN/agent.log"; }
agent X402Z_AGENT_ALLOWED_HOSTS=127.0.0.1 X402Z_AGENT_DAILY_CAP_ZAT=50000
agent X402Z_AGENT_ALLOWED_HOSTS=api.example.org X402Z_AGENT_DAILY_CAP_ZAT=50000

echo "stack up; merchant UA ${UA:0:16}…"
cat "$RUN/pids"
