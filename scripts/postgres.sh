#!/usr/bin/env bash
set -euo pipefail

CONTAINER_NAME="price-oracle-postgres"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-oracle}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-oracle}"
POSTGRES_DB="${POSTGRES_DB:-price_oracle}"

usage() {
    echo "Usage: $0 {create|stop|destroy|drop}"
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

drop_tables() {
    if ! docker ps --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
        echo "Container '${CONTAINER_NAME}' is not running."
        exit 1
    fi

    echo "Dropping all tables in database '${POSTGRES_DB}'..."
    docker exec -e PGPASSWORD="${POSTGRES_PASSWORD}" "${CONTAINER_NAME}" \
        psql -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" -v ON_ERROR_STOP=1 \
        -c "DROP SCHEMA public CASCADE; CREATE SCHEMA public;"
    echo "All tables dropped."
}

case "${1:-}" in
    create)      create      ;;
    stop)        stop        ;;
    destroy)     destroy     ;;
    drop) drop_tables ;;
    *)           usage       ;;
esac
