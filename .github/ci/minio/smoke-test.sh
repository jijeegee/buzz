#!/usr/bin/env bash
# Run on a disposable Docker host with the CI Compose files and a built image.
set -euo pipefail
: "${MINIO_CI_IMAGE:?Set MINIO_CI_IMAGE to the locally built image}"
: "${COMPOSE_FILE:?Select docker-compose.yml and docker-compose.ci.yml}"

# Exercise the real healthcheck and initializer without pulling over the image
# under test. No application services or tests need a MinIO build step.
docker compose up -d --wait --wait-timeout 90 --pull never minio
docker compose run --rm --no-deps --pull never minio-init
docker compose exec -T minio sh -eu -c '
  minio --version
  mc --version
  mc alias set local http://localhost:9000 kuzz_dev kuzz_dev_secret
  printf "kuzz-minio-smoke\n" > /tmp/expected
  mc cp /tmp/expected local/kuzz-media/smoke-test
  mc cat local/kuzz-media/smoke-test > /tmp/actual
  cmp /tmp/expected /tmp/actual
  status=$(curl --silent --show-error --output /dev/null --write-out "%{http_code}" \
    http://localhost:9000/kuzz-media/smoke-test)
  test "$status" = 403
  mc rm local/kuzz-media/smoke-test
  if mc stat local/kuzz-media/smoke-test; then
    echo "Deleted object is still present" >&2
    exit 1
  fi
'
