#!/usr/bin/env bash
set -euo pipefail

ELEMENTS_PORT="${ELEMENTS_PORT:-18884}"
ELEMENTS_RPC_USER="${ELEMENTS_RPC_USER:-elements}"
ELEMENTS_RPC_PASSWORD="${ELEMENTS_RPC_PASSWORD:-elements}"
if [[ "$(uname)" == "Darwin" ]]; then
    _DEFAULT_DATA_DIR="${HOME}/Library/Application Support/Elements"
else
    _DEFAULT_DATA_DIR="${HOME}/.elements"
fi
ELEMENTS_DATA_DIR="${ELEMENTS_DATA_DIR:-${_DEFAULT_DATA_DIR}}"
ELEMENTS_PID_FILE="${ELEMENTS_DATA_DIR}/elementsd.pid"

# elements-cli wrapper
ecli() {
    elements-cli \
        -datadir="${ELEMENTS_DATA_DIR}" \
        "$@"
}

usage() {
    cat <<EOF
Usage: $0 {start|stop|destroy|issue}

Commands:
  start              Start elementsd in regtest mode
  stop               Stop elementsd
  destroy            Stop elementsd and remove its data directory
  issue <address>    Issue 1 L-BTC to the provided address

Environment variables:
  ELEMENTS_PORT          RPC port (default: 18884)
  ELEMENTS_RPC_USER      RPC username (default: elements)
  ELEMENTS_RPC_PASSWORD  RPC password (default: elements)
  ELEMENTS_DATA_DIR      Data directory (default: ~/.elements-regtest)
EOF
    exit 1
}

start() {
    if ecli getblockchaininfo &>/dev/null; then
        echo "elementsd is already running on port ${ELEMENTS_PORT}"
        return
    fi

    mkdir -p "${ELEMENTS_DATA_DIR}"

    # Write config so elements-cli can auto-discover settings
    cat > "${ELEMENTS_DATA_DIR}/elements.conf" <<CONF
chain=elementsregtest

[elementsregtest]
rpcuser=${ELEMENTS_RPC_USER}
rpcpassword=${ELEMENTS_RPC_PASSWORD}
rpcport=${ELEMENTS_PORT}
txindex=1
validatepegin=0
fallbackfee=0.00001
CONF

    echo "Starting elementsd regtest on port ${ELEMENTS_PORT} (datadir: ${ELEMENTS_DATA_DIR})..."
    elementsd \
        -datadir="${ELEMENTS_DATA_DIR}" \
        -daemon \
        -pid="${ELEMENTS_PID_FILE}"

    echo "Waiting for elementsd to be ready..."
    for i in $(seq 1 30); do
        if ecli getblockchaininfo &>/dev/null; then
            echo "elementsd is running on port ${ELEMENTS_PORT}"
            return
        fi
        sleep 1
    done
    echo "Error: elementsd did not become ready in time."
    exit 1
}

stop() {
    echo "Stopping elementsd..."
    ecli stop 2>/dev/null || true

    # Wait for process to exit
    for i in $(seq 1 15); do
        if ! ecli getblockchaininfo &>/dev/null; then
            echo "elementsd stopped."
            return
        fi
        sleep 1
    done
    echo "Warning: elementsd may still be running."
}

destroy() {
    stop
    echo "Removing data directory '${ELEMENTS_DATA_DIR}'..."
    rm -rf "${ELEMENTS_DATA_DIR}"
    echo "Data directory removed."
}

issue() {
    local address="${1:-}"
    if [[ -z "${address}" ]]; then
        echo "Error: address is required."
        echo "Usage: $0 issue <address>"
        exit 1
    fi

    echo "Creating or loading wallet..."
    ecli createwallet "default" 2>/dev/null || ecli loadwallet "default" 2>/dev/null || true

    echo "Generating initial blocks..."
    local miner_addr
    miner_addr=$(ecli getnewaddress)
    ecli generatetoaddress 101 "${miner_addr}" > /dev/null

    echo "Issuing 1 L-BTC to ${address}..."
    local txid
    txid=$(ecli sendtoaddress "${address}" 1)
    echo "Sent in transaction: ${txid}"

    echo "Confirming transaction..."
    ecli generatetoaddress 1 "${miner_addr}" > /dev/null
    echo "Done. 1 L-BTC issued to ${address}"
}

case "${1:-}" in
    start)   start ;;
    stop)    stop ;;
    destroy) destroy ;;
    issue)   issue "${2:-}" ;;
    *)       usage ;;
esac
