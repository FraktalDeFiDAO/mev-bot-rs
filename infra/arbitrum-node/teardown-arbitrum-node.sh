#!/usr/bin/env bash
set -Eeuo pipefail

PROFILE="${AWS_PROFILE:-mev-bot}"
REGION="${AWS_REGION:-}"
INSTANCE_ID=""
DELETE_DATA=false
CONFIRM_VOLUME_ID=""
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
STATE_FILE="${ARBITRUM_STATE_FILE:-${SCRIPT_DIR}/arbitrum-node-resources.env}"

usage() {
  cat <<'EOF'
Usage: teardown-arbitrum-node.sh [--region REGION] [--profile PROFILE]
       [--instance-id i-...] [--delete-data-volume --confirm-volume-id vol-...]

Default behavior terminates only the Arbitrum EC2 instance and preserves its
persistent data volume. The data volume is deleted only when both destructive
options are supplied and the volume ID is typed again at the prompt.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --region) REGION="$2"; shift 2 ;;
    --profile) PROFILE="$2"; shift 2 ;;
    --instance-id) INSTANCE_ID="$2"; shift 2 ;;
    --delete-data-volume) DELETE_DATA=true; shift ;;
    --confirm-volume-id) CONFIRM_VOLUME_ID="$2"; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

STATE_INSTANCE_ID=""
STATE_VOLUME_ID=""
STATE_REGION=""
if [[ -r "$STATE_FILE" ]]; then
  STATE_INSTANCE_ID="$(sed -n 's/^INSTANCE_ID=//p' "$STATE_FILE" | head -n1)"
  STATE_VOLUME_ID="$(sed -n 's/^DATA_VOLUME_ID=//p' "$STATE_FILE" | head -n1)"
  STATE_REGION="$(sed -n 's/^REGION=//p' "$STATE_FILE" | head -n1)"
fi
if [[ -z "$REGION" && -n "$STATE_REGION" ]]; then
  REGION="$STATE_REGION"
fi
if [[ -z "$REGION" ]]; then
  REGION="$(aws configure get region --profile "$PROFILE" 2>/dev/null || true)"
fi
if [[ -z "$REGION" ]]; then
  REGION=us-east-1
fi

awsr() { aws --profile "$PROFILE" --region "$REGION" "$@"; }

if [[ -z "$INSTANCE_ID" && -n "$STATE_INSTANCE_ID" ]]; then
  # The local resource state contains IDs only; no credentials or secrets.
  INSTANCE_ID="$STATE_INSTANCE_ID"
fi
if [[ -z "$INSTANCE_ID" ]]; then
  matches="$(awsr ec2 describe-instances \
    --filters Name=tag:Name,Values=arbitrum-one-node Name=tag:Project,Values=LiquidationBot \
      Name=tag:Role,Values=NitroFullNode Name=instance-state-name,Values=pending,running,stopping,stopped \
    --query 'Reservations[].Instances[].InstanceId' --output text)"
  read -r -a ids <<<"$matches"
  if [[ ${#ids[@]} -ne 1 || -z "${ids[0]:-}" ]]; then
    echo "Expected exactly one non-terminated instance tagged Name=arbitrum-one-node; found ${#ids[@]}." >&2
    exit 1
  fi
  INSTANCE_ID="${ids[0]}"
fi

instance="$(awsr ec2 describe-instances --instance-ids "$INSTANCE_ID" --output json)"
if ! jq -e '
  .Reservations[0].Instances[0] as $i |
  ($i.Tags // []) as $t |
  $i.InstanceId != null and
  any($t[]; .Key == "Name" and .Value == "arbitrum-one-node") and
  any($t[]; .Key == "Project" and .Value == "LiquidationBot") and
  any($t[]; .Key == "Role" and .Value == "NitroFullNode")
' >/dev/null <<<"$instance"; then
  echo "Refusing to terminate ${INSTANCE_ID}: it is not the tagged LiquidationBot Arbitrum Nitro node." >&2
  exit 1
fi
volume_id="$(jq -r '.Reservations[0].Instances[0].BlockDeviceMappings[]? | select(.DeviceName == "/dev/sdf") | .Ebs.VolumeId' <<<"$instance" | head -n1)"
if [[ -z "$volume_id" || "$volume_id" == "null" ]]; then
  volume_id="$STATE_VOLUME_ID"
fi
instance_state="$(jq -r '.Reservations[0].Instances[0].State.Name // "not-found"' <<<"$instance")"
if [[ "$instance_state" == "not-found" ]]; then
  echo "Instance ${INSTANCE_ID} was not found in ${REGION}." >&2
  exit 1
fi

echo "Instance to terminate: ${INSTANCE_ID} (${instance_state}) in ${REGION}"
echo "Persistent data volume to preserve: ${volume_id:-not attached}"
if [[ "$DELETE_DATA" == true ]]; then
  if [[ -z "$volume_id" || "$CONFIRM_VOLUME_ID" != "$volume_id" ]]; then
    echo "For explicit data deletion, also pass --confirm-volume-id ${volume_id:-<volume-id>}." >&2
    exit 2
  fi
  volume_info="$(awsr ec2 describe-volumes --volume-ids "$volume_id" --output json)"
  if ! jq -e '
    .Volumes[0] as $v |
    ($v.Tags // []) as $t |
    $v.VolumeId != null and $v.Encrypted == true and $v.VolumeType == "gp3" and
    any($t[]; .Key == "Name" and .Value == "arbitrum-one-node-data") and
    any($t[]; .Key == "Project" and .Value == "LiquidationBot") and
    any($t[]; .Key == "Role" and .Value == "NitroFullNode")
  ' >/dev/null <<<"$volume_info"; then
    echo "Refusing to delete ${volume_id}: it is not a verified encrypted gp3 Arbitrum data volume." >&2
    exit 1
  fi
  read -r -p "Type DELETE ${volume_id} to permanently delete the Arbitrum database: " typed
  if [[ "$typed" != "DELETE ${volume_id}" ]]; then
    echo "Confirmation did not match; no resources changed." >&2
    exit 1
  fi
else
  echo "The data volume will be preserved (no --delete-data-volume flag)."
fi

if [[ "$instance_state" != "terminated" && "$instance_state" != "shutting-down" ]]; then
  awsr ec2 terminate-instances --instance-ids "$INSTANCE_ID" >/dev/null
  awsr ec2 wait instance-terminated --instance-ids "$INSTANCE_ID"
fi

if [[ "$DELETE_DATA" == true && -n "$volume_id" ]]; then
  for _ in $(seq 1 60); do
    volume_state="$(awsr ec2 describe-volumes --volume-ids "$volume_id" --query 'Volumes[0].State' --output text 2>/dev/null || true)"
    [[ "$volume_state" == "available" ]] && break
    sleep 5
  done
  if [[ "${volume_state:-}" != "available" ]]; then
    echo "Volume ${volume_id} is not available for deletion (state=${volume_state:-unknown}); it has been preserved." >&2
    exit 1
  fi
  awsr ec2 delete-volume --volume-id "$volume_id"
  echo "Deleted persistent data volume ${volume_id}."
else
  echo "EC2 instance terminated; persistent data volume ${volume_id:-not attached} remains preserved."
fi
