#!/usr/bin/env bash
set -Eeuo pipefail

readonly IMAGE="offchainlabs/nitro-node:v3.12.1-70fa99a"
readonly CONTAINER="arbitrum-nitro"
readonly DATA_DIR="/data/arbitrum"
readonly CONFIG_FILE="/etc/arbitrum/nitro-config.json"
readonly DIRECT_ENV_FILE="/etc/arbitrum/nitro.env"
readonly RUNTIME_ENV_DIR="/run/arbitrum"
readonly RUNTIME_ENV_FILE="${RUNTIME_ENV_DIR}/nitro.env"

mountpoint -q "$DATA_DIR" || { echo 'Nitro is not starting: persistent data volume is not mounted.' >&2; exit 1; }
[[ -f "${DATA_DIR}/snapshot-import-complete.json" ]] || { echo 'Verified pruned snapshot import has not completed.' >&2; exit 1; }

rpc_url="${NITRO_PARENT__CHAIN_CONNECTION_URL:-${ETHEREUM_RPC_URL:-}}"
beacon_url="${NITRO_PARENT__CHAIN_BLOB__CLIENT_BEACON__URL:-${ETHEREUM_BEACON_URL:-}}"

if [[ -z "$rpc_url" || -z "$beacon_url" ]]; then
  metadata_token="$(curl --fail --silent --show-error --max-time 3 \
    -H "X-aws-ec2-metadata-token-ttl-seconds: 21600" \
    -X PUT http://169.254.169.254/latest/api/token 2>/dev/null || true)"
  region=""
  if [[ -n "$metadata_token" ]]; then
    region="$(curl --fail --silent --show-error --max-time 3 \
      -H "X-aws-ec2-metadata-token: ${metadata_token}" \
      http://169.254.169.254/latest/meta-data/placement/region 2>/dev/null || true)"
  fi
  unset metadata_token

  if [[ -z "$rpc_url" && -n "${NITRO_SSM_PARENT_RPC_PARAMETER:-}" && -n "$region" ]]; then
    rpc_url="$(aws ssm get-parameter --region "$region" \
      --name "$NITRO_SSM_PARENT_RPC_PARAMETER" --with-decryption \
      --query 'Parameter.Value' --output text 2>/dev/null || true)"
  fi
  if [[ -z "$beacon_url" && -n "${NITRO_SSM_BEACON_RPC_PARAMETER:-}" && -n "$region" ]]; then
    beacon_url="$(aws ssm get-parameter --region "$region" \
      --name "$NITRO_SSM_BEACON_RPC_PARAMETER" --with-decryption \
      --query 'Parameter.Value' --output text 2>/dev/null || true)"
  fi
fi

if [[ -z "$rpc_url" || -z "$beacon_url" || "$rpc_url" == "None" || "$beacon_url" == "None" ]]; then
  echo "Nitro is not starting: configure both Ethereum parent-chain RPC and beacon URLs in ${DIRECT_ENV_FILE} or the configured SSM SecureString parameters."
  exit 0
fi

if [[ "$rpc_url" == *[$'\n\r']* || "$beacon_url" == *[$'\n\r']* ]]; then
  echo "Nitro is not starting: RPC URLs must be single-line values."
  exit 1
fi

install -d -o root -g root -m 0700 "$RUNTIME_ENV_DIR"
umask 077
{
  printf 'NITRO_PARENT__CHAIN_CONNECTION_URL=%s\n' "$rpc_url"
  printf 'NITRO_PARENT__CHAIN_BLOB__CLIENT_BEACON__URL=%s\n' "$beacon_url"
} > "$RUNTIME_ENV_FILE"
unset rpc_url beacon_url

exec /usr/bin/docker run --rm --name "$CONTAINER" \
  --pull never \
  --stop-timeout 1800 \
  --ulimit nofile=1048576:1048576 \
  --log-driver json-file \
  --log-opt max-size=100m \
  --log-opt max-file=10 \
  --env-file "$RUNTIME_ENV_FILE" \
  --publish 127.0.0.1:8547:8547 \
  --publish 127.0.0.1:8548:8548 \
  --volume "${DATA_DIR}:/home/user/.arbitrum" \
  --volume "${CONFIG_FILE}:/home/user/nitro-config.json:ro" \
  "$IMAGE" \
  --conf.file /home/user/nitro-config.json \
  --conf.env-prefix NITRO
