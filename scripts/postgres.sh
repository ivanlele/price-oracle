#!/usr/bin/env bash
set -euo pipefail

CONTAINER_NAME="price-oracle-postgres"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-oracle}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-oracle}"
POSTGRES_DB="${POSTGRES_DB:-price_oracle}"

usage() {
    echo "Usage: $0 {create|stop|destroy}"
    exit 1
}

create() {
    if docker ps -a --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
        echo "Container '${CONTAINER_NAME}' already exists."
        if ! docker ps --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
            echo "Starting existing container..."
            docker start "${CONTAINER_NAME}"
        fi
    else
        echo "Creating PostgreSQL container '${CONTAINER_NAME}' on port ${POSTGRES_PORT}..."
        docker run -d \
            --name "${CONTAINER_NAME}" \
            -e POSTGRES_USER="${POSTGRES_USER}" \
            -e POSTGRES_PASSWORD="${POSTGRES_PASSWORD}" \
            -e POSTGRES_DB="${POSTGRES_DB}" \
            -p "${POSTGRES_PORT}:5432" \
            postgres:16-alpine
    fi
    echo "PostgreSQL is running on port ${POSTGRES_PORT}"
}

stop() {
    echo "Stopping container '${CONTAINER_NAME}'..."
    docker stop "${CONTAINER_NAME}"
    echo "Container stopped."
}

destroy() {
    echo "Destroying container '${CONTAINER_NAME}'..."
    docker rm -f "${CONTAINER_NAME}"
    echo "Container destroyed."
}

case "${1:-}" in
    create)  create  ;;
    stop)    stop    ;;
    destroy) destroy ;;
    *)       usage   ;;
esac
