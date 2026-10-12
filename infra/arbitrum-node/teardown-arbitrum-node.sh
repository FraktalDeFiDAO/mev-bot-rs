#!/usr/bin/env bash
set -Eeuo pipefail

PROFILE="${AWS_PROFILE:-mev-bot}"
REGION="${AWS_REGION:-${AWS_DEFAULT_REGION:-}}"
STATE_FILE="${ARBITRUM_STATE_FILE:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/arbitrum-node-resources.env}"
DELETE_DATA=false
ASSUME_YES=false
CONFIRM_VOLUME_ID=""
export AWS_PAGER=""

usage() {
  cat <<'EOF'
Usage: teardown-arbitrum-node.sh [--region REGION] [--profile PROFILE]
       [--yes] [--delete-data-volume --confirm-volume-id vol-...]

Default: gracefully stop Nitro when SSM is available, terminate the EC2 node,
and preserve the tagged Arbitrum data volume. Data deletion requires both the
explicit --delete-data-volume flag and the exact --confirm-volume-id value.
Without --yes, the script asks for the instance ID before termination.
EOF
}
fail() { echo "ERROR: $*" >&2; exit 1; }
while [[ $# -gt 0 ]]; do
  case "$1" in
    --region) REGION="$2"; shift 2 ;;
    --profile) PROFILE="$2"; shift 2 ;;
    --yes) ASSUME_YES=true; shift ;;
    --delete-data-volume) DELETE_DATA=true; shift ;;
    --confirm-volume-id) CONFIRM_VOLUME_ID="$2"; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) fail "Unknown option: $1" ;;
  esac
done
[[ -n "$REGION" ]] || REGION="$(aws configure get region --profile "$PROFILE" 2>/dev/null || true)"
[[ -n "$REGION" ]] || fail 'Set --region or configure an AWS region.'
if [[ "$DELETE_DATA" == true ]]; then
  [[ "$CONFIRM_VOLUME_ID" =~ ^vol-[0-9a-f]+$ ]] || fail '--delete-data-volume requires --confirm-volume-id vol-...'
elif [[ -n "$CONFIRM_VOLUME_ID" ]]; then
  fail '--confirm-volume-id is only valid with --delete-data-volume.'
fi
awsr() { aws --profile "$PROFILE" --region "$REGION" "$@"; }
command -v aws >/dev/null || fail 'AWS CLI is required.'
command -v jq >/dev/null || fail 'jq is required.'

if [[ -r "$STATE_FILE" ]]; then
  saved_region="$(sed -n 's/^REGION=//p' "$STATE_FILE" | head -n1)"
  [[ -z "$saved_region" || "$saved_region" == "$REGION" ]] || fail "State file is for ${saved_region}; pass --region ${saved_region}."
fi

instances="$(awsr ec2 describe-instances --filters \
  'Name=tag:Name,Values=arbitrum-one-node' 'Name=tag:Project,Values=LiquidationBot' \
  'Name=tag:Role,Values=NitroFullNode' 'Name=tag:Chain,Values=ArbitrumOne' \
  'Name=tag:ManagedBy,Values=aws-cli' 'Name=instance-state-name,Values=pending,running,stopping,stopped' --output json)"
instance_count="$(jq '[.Reservations[].Instances[]] | length' <<<"$instances")"
(( instance_count <= 1 )) || fail "Found ${instance_count} tagged EC2 instances; refusing ambiguous teardown."
INSTANCE_ID="$(jq -r '.Reservations[0].Instances[0].InstanceId // empty' <<<"$instances")"
if [[ -z "$INSTANCE_ID" && -r "$STATE_FILE" ]]; then
  INSTANCE_ID="$(sed -n 's/^INSTANCE_ID=//p' "$STATE_FILE" | head -n1)"
  [[ "$INSTANCE_ID" =~ ^i-[0-9a-f]+$ ]] || INSTANCE_ID=""
fi

VOLUMES="$(awsr ec2 describe-volumes --filters \
  'Name=tag:Name,Values=arbitrum-one-node-data' 'Name=tag:Project,Values=LiquidationBot' \
  'Name=tag:Role,Values=NitroFullNode' --output json)"
volume_count="$(jq '.Volumes | length' <<<"$VOLUMES")"
(( volume_count <= 1 )) || fail "Found ${volume_count} tagged data volumes; refusing ambiguous teardown."
DATA_VOLUME_ID="$(jq -r '.Volumes[0].VolumeId // empty' <<<"$VOLUMES")"
if [[ -n "$INSTANCE_ID" ]]; then
  instance_json="$(awsr ec2 describe-instances --instance-ids "$INSTANCE_ID" --output json)"
  jq -e '.Reservations[0].Instances[0].Tags as $t |
    any($t[]; .Key == "Name" and .Value == "arbitrum-one-node") and
    any($t[]; .Key == "Project" and .Value == "LiquidationBot") and
    any($t[]; .Key == "Role" and .Value == "NitroFullNode") and
    any($t[]; .Key == "Chain" and .Value == "ArbitrumOne") and
    any($t[]; .Key == "ManagedBy" and .Value == "aws-cli")' >/dev/null <<<"$instance_json" \
    || fail "Instance ${INSTANCE_ID} is not owned by this deployment; no instance action taken."
  # Preserve every non-root disk, including the retained migration cache disk.
  jq -e '.Reservations[0].Instances[0] | .RootDeviceName as $root |
    [.BlockDeviceMappings[] | select(.DeviceName != $root) | .Ebs.DeleteOnTermination] | all(. == false)' >/dev/null <<<"$instance_json" \
    || fail 'A non-root disk has DeleteOnTermination enabled; disable it before teardown.'
  actual_volume="$DATA_VOLUME_ID"
  if [[ -n "$DATA_VOLUME_ID" ]]; then
    jq -e --arg id "$DATA_VOLUME_ID" '.Reservations[0].Instances[0].BlockDeviceMappings | any(.Ebs.VolumeId == $id)' >/dev/null <<<"$instance_json" \
      || fail 'Canonical tagged data volume is not attached to this instance.'
  fi
fi
if [[ "$DELETE_DATA" == true ]]; then
  [[ -n "$DATA_VOLUME_ID" && "$DATA_VOLUME_ID" == "$CONFIRM_VOLUME_ID" ]] \
    || fail 'Explicit data-volume confirmation does not match the deployment; no instance action taken.'
fi

if [[ -z "$INSTANCE_ID" ]]; then
  echo 'No active tagged EC2 instance found.'
else
  state="$(awsr ec2 describe-instances --instance-ids "$INSTANCE_ID" --query 'Reservations[0].Instances[0].State.Name' --output text)"
  [[ "$state" != terminated ]] || { echo "Instance ${INSTANCE_ID} is already terminated."; INSTANCE_ID=""; }
fi

if [[ -n "$INSTANCE_ID" ]]; then
  echo "Planned teardown: EC2 ${INSTANCE_ID} in ${REGION}; persistent data EBS ${DATA_VOLUME_ID:-not found} will be preserved by default."
  if [[ "$ASSUME_YES" != true ]]; then
    [[ -t 0 ]] || fail 'Non-interactive teardown requires --yes.'
    read -r -p "Type ${INSTANCE_ID} to terminate this instance: " answer
    [[ "$answer" == "$INSTANCE_ID" ]] || fail 'Confirmation did not match; no instance action taken.'
  fi
  if [[ "$state" == running ]]; then
    managed="$(awsr ssm describe-instance-information --filters "Key=InstanceIds,Values=${INSTANCE_ID}" \
      --query 'InstanceInformationList[0].InstanceId' --output text 2>/dev/null || true)"
    if [[ "$managed" == "$INSTANCE_ID" ]]; then
      echo 'Requesting graceful Nitro service shutdown through SSM.'
      command_id="$(awsr ssm send-command --instance-ids "$INSTANCE_ID" --document-name AWS-RunShellScript \
        --parameters 'commands=["set -e; systemctl stop arbitrum-activate.timer arbitrum-activate.service arbitrum-health.timer arbitrum-health.service arbitrum-nitro.service arbitrum-snapshot-init.service; systemctl stop arbitrum-snapshot-prefetch.service 2>/dev/null || true"]' --query 'Command.CommandId' --output text)"
      shutdown_ok=false
      for _ in $(seq 1 390); do
        invocation="$(awsr ssm get-command-invocation --command-id "$command_id" --instance-id "$INSTANCE_ID" --output json 2>/dev/null || true)"
        status="$(jq -r '.Status // "Pending"' <<<"$invocation")"
        if [[ "$status" == Success ]]; then shutdown_ok=true; break; fi
        if [[ "$status" == Failed || "$status" == Cancelled || "$status" == TimedOut ]]; then
          jq -r '.StandardErrorContent // .StandardOutputContent // empty' <<<"$invocation" >&2
          break
        fi
        sleep 5
      done
      [[ "$shutdown_ok" == true ]] || fail 'Graceful Nitro shutdown did not complete; instance was not terminated. Re-run after diagnosing or use AWS CLI termination manually if required.'
    else
      echo 'SSM is not online; proceeding with the normal EC2 OS shutdown path.'
    fi
  fi
  awsr ec2 terminate-instances --instance-ids "$INSTANCE_ID" >/dev/null
  awsr ec2 wait instance-terminated --instance-ids "$INSTANCE_ID"
  echo "EC2 ${INSTANCE_ID} terminated."
  if [[ -n "$DATA_VOLUME_ID" && "${actual_volume:-}" == "$DATA_VOLUME_ID" ]]; then
    awsr ec2 wait volume-available --volume-ids "$DATA_VOLUME_ID"
  fi
fi

if [[ -z "$DATA_VOLUME_ID" ]]; then
  echo 'No tagged persistent data volume found.'
  exit 0
fi
volume="$(awsr ec2 describe-volumes --volume-ids "$DATA_VOLUME_ID" --output json)"
tagged="$(jq -r '.Volumes[0] as $v | (($v.Tags // []) | any(.[]; .Key == "Project" and .Value == "LiquidationBot")) and (($v.Tags // []) | any(.[]; .Key == "Role" and .Value == "NitroFullNode")) and (($v.Tags // []) | any(.[]; .Key == "Name" and .Value == "arbitrum-one-node-data"))' <<<"$volume")"
encrypted="$(jq -r '.Volumes[0].Encrypted' <<<"$volume")"
state="$(jq -r '.Volumes[0].State' <<<"$volume")"
[[ "$tagged" == true && "$encrypted" == true ]] || fail "Volume ${DATA_VOLUME_ID} tags/encryption do not match; preserving it."
if [[ "$DELETE_DATA" != true ]]; then
  echo "PRESERVED_DATA_VOLUME_ID=${DATA_VOLUME_ID} STATE=${state} SIZE_GIB=$(jq -r '.Volumes[0].Size' <<<"$volume")"
  exit 0
fi
[[ "$DATA_VOLUME_ID" == "$CONFIRM_VOLUME_ID" ]] || fail "Volume ID confirmation does not match ${DATA_VOLUME_ID}; preserving it."
[[ "$state" == available ]] || fail "Volume ${DATA_VOLUME_ID} is ${state}; it must be detached and available before deletion."
echo "DESTRUCTIVE ACTION: delete encrypted Arbitrum data volume ${DATA_VOLUME_ID} ($(jq -r '.Volumes[0].Size' <<<"$volume") GiB) in ${REGION}."
if [[ "$ASSUME_YES" != true ]]; then
  [[ -t 0 ]] || fail 'Non-interactive data deletion requires --yes in addition to both explicit volume flags.'
  read -r -p "Type DELETE ${DATA_VOLUME_ID} to permanently delete blockchain data: " answer
  [[ "$answer" == "DELETE ${DATA_VOLUME_ID}" ]] || fail 'Confirmation did not match; data volume preserved.'
fi
awsr ec2 delete-volume --volume-id "$DATA_VOLUME_ID"
echo "Deleted ${DATA_VOLUME_ID}."
