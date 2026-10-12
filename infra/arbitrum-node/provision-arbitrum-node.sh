#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROFILE="${AWS_PROFILE:-mev-bot}"
REGION="${AWS_REGION:-${AWS_DEFAULT_REGION:-}}"
ADMIN_CIDR="${ADMIN_CIDR:-}"
KEY_NAME="${ARBITRUM_KEY_NAME:-arbitrum-one-node-key}"
KEY_PATH="${ARBITRUM_KEY_PATH:-${HOME}/.ssh/arbitrum-one-node-key.pem}"
STATE_FILE="${ARBITRUM_STATE_FILE:-${SCRIPT_DIR}/arbitrum-node-resources.env}"
INSTANCE_NAME=arbitrum-one-node
DATA_VOLUME_NAME=arbitrum-one-node-data
SG_NAME=arbitrum-one-node-sg
ROLE_NAME=arbitrum-one-node-ssm-role
PROFILE_NAME=arbitrum-one-node-ssm-profile
# The 2026-09-23 pruned snapshot is 2,466.67 GiB; Nitro's multipart join
# temporarily requires twice that space. A 6 TiB disk leaves safe headroom.
DATA_SIZE=4096
DATA_IOPS=16000
DATA_THROUGHPUT=1000
ROOT_SIZE=50
NITRO_IMAGE=offchainlabs/nitro-node:v3.12.1-70fa99a
WAIT_FOR_QUOTA=false
export AWS_PAGER=""

usage() {
  cat <<'EOF'
Usage: provision-arbitrum-node.sh [--region REGION] [--profile PROFILE] [--admin-cidr IPv4/32]
       [--wait-for-quota]

Uses a suitable existing VPC or creates a small tagged public VPC. Reads
ETHEREUM_RPC_URL and ETHEREUM_BEACON_URL from the environment and stores them
as scoped SSM SecureStrings only when both are set. Existing parameters are
preserved unless ARBITRUM_OVERWRITE_SSM_PARAMETERS=1.
With --wait-for-quota, waits for sufficient Standard On-Demand vCPU quota
without stopping or modifying any existing workload.
EOF
}
while [[ $# -gt 0 ]]; do
  case "$1" in
    --region) REGION="$2"; shift 2 ;;
    --profile) PROFILE="$2"; shift 2 ;;
    --admin-cidr) ADMIN_CIDR="$2"; shift 2 ;;
    --wait-for-quota) WAIT_FOR_QUOTA=true; shift ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done
if [[ -z "$REGION" ]]; then REGION="$(aws configure get region --profile "$PROFILE" 2>/dev/null || true)"; fi
[[ -n "$REGION" ]] || REGION=us-east-1
awsr() { aws --profile "$PROFILE" --region "$REGION" "$@"; }
awsiam() { aws --profile "$PROFILE" "$@"; }
fail() { echo "ERROR: $*" >&2; exit 1; }
command -v aws >/dev/null || fail 'AWS CLI is required.'
command -v jq >/dev/null || fail 'jq is required.'
command -v curl >/dev/null || fail 'curl is required.'
command -v python3 >/dev/null || fail 'python3 is required.'
command -v flock >/dev/null || fail 'flock is required to prevent concurrent provisioning.'
umask 077
exec {deployment_lock}>"${STATE_FILE}.lock"
flock -n "$deployment_lock" || fail 'Another provisioning process holds the deployment lock.'

identity="$(awsiam sts get-caller-identity --output json)"
ACCOUNT_ID="$(jq -r '.Account' <<<"$identity")"
CALLER_ARN="$(jq -r '.Arn' <<<"$identity")"
echo "AWS account: ${ACCOUNT_ID} (${CALLER_ARN})"
echo "Selected region: ${REGION}"

# A current AWS Free plan blocks paid EC2 instance types at RunInstances time.
# Detect this before creating/modifying networking or IAM resources. The plan
# upgrade is account-wide and is deliberately left to the account owner.
plan_error_file="$(mktemp)"
if FREE_PLAN_STATE="$(aws --profile "$PROFILE" --region us-east-1 freetier get-account-plan-state --output json 2>"$plan_error_file")"; then
  ACCOUNT_PLAN="$(jq -r '.accountPlanType // empty' <<<"$FREE_PLAN_STATE")"
  ACCOUNT_PLAN_STATUS="$(jq -r '.accountPlanStatus // empty' <<<"$FREE_PLAN_STATE")"
  if [[ "$ACCOUNT_PLAN" == FREE ]]; then
    fail 'This AWS account is on the FREE plan, which blocks the requested paid EC2 instance. For the AWS project signup experience, complete the approved upgrade at https://settings.aws.com under Billing > Upgrade.'
  fi
  [[ "$ACCOUNT_PLAN" == PAID && "$ACCOUNT_PLAN_STATUS" == ACTIVE ]] \
    || fail "AWS account plan is ${ACCOUNT_PLAN:-unknown}/${ACCOUNT_PLAN_STATUS:-unknown}; a PAID/ACTIVE plan is required for the requested On-Demand node."
elif grep -q ResourceNotFoundException "$plan_error_file" && [[ "${ARBITRUM_CONFIRMED_PAID_ACCOUNT:-}" == "$ACCOUNT_ID" ]]; then
  echo 'Plan API has no account record; using previously verified Paid activation for this exact account. EC2 will enforce eligibility at launch.'
else
  cat "$plan_error_file" >&2
  fail 'Could not verify account plan. ARBITRUM_CONFIRMED_PAID_ACCOUNT may name an account whose Paid activation was already verified, only when the API returns ResourceNotFoundException.'
fi
rm -- "$plan_error_file"

# This is also used when a tagged instance already exists. Supplying endpoints
# on a rerun updates configuration without launching or restarting a node.
store_parameter() (
  local name="$1" env_name="$2" prior="" overwrite=false parameter_file
  prior="$(awsr ssm get-parameter --name "$name" --query 'Parameter.Name' --output text 2>/dev/null || true)"
  if [[ "$prior" != None && -n "$prior" ]]; then
    if [[ "${ARBITRUM_OVERWRITE_SSM_PARAMETERS:-0}" != 1 ]]; then echo "Preserving existing SSM parameter ${name}."; return 0; fi
    overwrite=true
  fi
  parameter_file="$(mktemp)"
  trap 'rm -- "$parameter_file"' EXIT
  python3 - "$name" "$env_name" "$parameter_file" "$overwrite" <<'PY'
import json, os, sys
name, env_name, path, overwrite = sys.argv[1:]
value = os.environ.get(env_name, "")
if not value or "\n" in value or "\r" in value:
    raise SystemExit(f"{env_name} must be a nonempty single-line URL")
with open(path, "w", encoding="utf-8") as f:
    json.dump({"Name": name, "Type": "SecureString", "Value": value,
               "Overwrite": overwrite == "true", "Tier": "Standard"}, f)
PY
  chmod 0600 "$parameter_file"
  awsr ssm put-parameter --cli-input-json "file://${parameter_file}" >/dev/null
  echo "Stored ${env_name} as a scoped SSM SecureString."
)
store_parent_endpoints() {
  if [[ -n "${ETHEREUM_RPC_URL:-}" && -n "${ETHEREUM_BEACON_URL:-}" ]]; then
    store_parameter /mev-bot/arbitrum/ethereum-rpc-url ETHEREUM_RPC_URL
    store_parameter /mev-bot/arbitrum/ethereum-beacon-url ETHEREUM_BEACON_URL
  elif [[ -n "${ETHEREUM_RPC_URL:-}" || -n "${ETHEREUM_BEACON_URL:-}" ]]; then
    echo 'Only one parent endpoint is set; snapshot preparation can run, but live RPC/catch-up requires both.'
  fi
}

# Reuse a tagged instance; never launch a second one on a simple rerun.
existing="$(awsr ec2 describe-instances --filters \
  "Name=tag:Name,Values=${INSTANCE_NAME}" Name=tag:Project,Values=LiquidationBot \
  Name=tag:Role,Values=NitroFullNode Name=instance-state-name,Values=pending,running,stopping,stopped --output json)"
existing_ids="$(jq -r '.Reservations[].Instances[].InstanceId' <<<"$existing")"
if [[ -n "$existing_ids" ]]; then
  readarray -t ids <<<"$existing_ids"
  [[ ${#ids[@]} -eq 1 ]] || fail 'Multiple active tagged Nitro instances exist; refusing to choose one.'
  INSTANCE_ID="${ids[0]}"
  details="$(awsr ec2 describe-instances --instance-ids "$INSTANCE_ID" --output json)"
  INSTANCE_TYPE="$(jq -r '.Reservations[0].Instances[0].InstanceType' <<<"$details")"
  AZ="$(jq -r '.Reservations[0].Instances[0].Placement.AvailabilityZone' <<<"$details")"
  PUBLIC_IP="$(jq -r '.Reservations[0].Instances[0].PublicIpAddress // ""' <<<"$details")"
  PRIVATE_IP="$(jq -r '.Reservations[0].Instances[0].PrivateIpAddress // ""' <<<"$details")"
  SG_ID="$(jq -r '.Reservations[0].Instances[0].SecurityGroups[0].GroupId // ""' <<<"$details")"
  VPC_ID="$(jq -r '.Reservations[0].Instances[0].VpcId' <<<"$details")"
  SUBNET_ID="$(jq -r '.Reservations[0].Instances[0].SubnetId' <<<"$details")"
  ROOT_VOLUME_ID="$(jq -r '.Reservations[0].Instances[0] | .RootDeviceName as $root | .BlockDeviceMappings[]? | select(.DeviceName == $root) | .Ebs.VolumeId' <<<"$details")"
  current_data="$(awsr ec2 describe-volumes --filters "Name=tag:Name,Values=${DATA_VOLUME_NAME}" Name=tag:Project,Values=LiquidationBot Name=tag:Role,Values=NitroFullNode --output json)"
  [[ "$(jq '.Volumes | length' <<<"$current_data")" == 1 ]] || fail 'Expected exactly one canonical tagged persistent data volume.'
  DATA_VOLUME_ID="$(jq -r '.Volumes[0].VolumeId' <<<"$current_data")"
  DATA_DEVICE_NAME="$(jq -r --arg id "$DATA_VOLUME_ID" '.Reservations[0].Instances[0].BlockDeviceMappings[]? | select(.Ebs.VolumeId == $id) | .DeviceName' <<<"$details")"
  [[ -n "$DATA_DEVICE_NAME" ]] || fail 'Canonical tagged data volume is not attached to the existing node.'
  jq -e --arg id "$DATA_VOLUME_ID" '.Reservations[0].Instances[0].BlockDeviceMappings[] | select(.Ebs.VolumeId == $id) | .Ebs.DeleteOnTermination == false' >/dev/null <<<"$details" || fail 'Persistent data volume must have DeleteOnTermination=false.'
  STATE="$(jq -r '.Reservations[0].Instances[0].State.Name' <<<"$details")"
  printf 'INSTANCE_ID=%s\nINSTANCE_TYPE=%s\nREGION=%s\nAZ=%s\nPUBLIC_IP=%s\nPRIVATE_IP=%s\nDATA_VOLUME_ID=%s\nSECURITY_GROUP_ID=%s\nVPC_ID=%s\nSUBNET_ID=%s\nROOT_VOLUME_ID=%s\nNITRO_IMAGE=%s\n' \
    "$INSTANCE_ID" "$INSTANCE_TYPE" "$REGION" "$AZ" "$PUBLIC_IP" "$PRIVATE_IP" "$DATA_VOLUME_ID" "$SG_ID" "$VPC_ID" "$SUBNET_ID" "$ROOT_VOLUME_ID" "$NITRO_IMAGE" >"$STATE_FILE"
  chmod 0600 "$STATE_FILE"
  printf 'DATA_DEVICE_NAME=%s\nDATA_MOUNT=/data/arbitrum\n' "$DATA_DEVICE_NAME" >>"$STATE_FILE"
  store_parent_endpoints
  echo "Reusing existing ${STATE} instance; no duplicate resources created."
  echo "INSTANCE_ID=${INSTANCE_ID}"
  echo "INSTANCE_TYPE=${INSTANCE_TYPE}"
  echo "REGION=${REGION} AZ=${AZ} PUBLIC_IP=${PUBLIC_IP} PRIVATE_IP=${PRIVATE_IP}"
  echo "DATA_VOLUME_ID=${DATA_VOLUME_ID} SECURITY_GROUP_ID=${SG_ID}"
  echo 'RPC_HTTP=http://127.0.0.1:8547 RPC_WS=ws://127.0.0.1:8548'
  exit 0
fi

# Detect a saved data disk before choosing AZ or constructing a block mapping.
volume_json="$(awsr ec2 describe-volumes --filters \
  "Name=tag:Name,Values=${DATA_VOLUME_NAME}" Name=tag:Project,Values=LiquidationBot \
  Name=tag:Role,Values=NitroFullNode --output json)"
volume_count="$(jq '.Volumes | length' <<<"$volume_json")"
(( volume_count <= 1 )) || fail "Found ${volume_count} tagged persistent volumes; refusing to create another."
REUSE_DATA=false
DATA_VOLUME_ID=""
DATA_AZ=""
if (( volume_count == 1 )); then
  REUSE_DATA=true
  DATA_VOLUME_ID="$(jq -r '.Volumes[0].VolumeId' <<<"$volume_json")"
  DATA_AZ="$(jq -r '.Volumes[0].AvailabilityZone' <<<"$volume_json")"
  volume_state="$(jq -r '.Volumes[0].State' <<<"$volume_json")"
  volume_type="$(jq -r '.Volumes[0].VolumeType' <<<"$volume_json")"
  volume_size="$(jq -r '.Volumes[0].Size' <<<"$volume_json")"
  volume_iops="$(jq -r '.Volumes[0].Iops // 0' <<<"$volume_json")"
  volume_throughput="$(jq -r '.Volumes[0].Throughput // 0' <<<"$volume_json")"
  volume_encrypted="$(jq -r '.Volumes[0].Encrypted' <<<"$volume_json")"
  [[ "$volume_state" == available ]] || fail "Persistent volume ${DATA_VOLUME_ID} is ${volume_state}; inspect attachment before retrying."
  [[ "$volume_type" == gp3 && "$volume_size" -ge 4096 && "$volume_iops" -ge 12000 && "$volume_throughput" -ge 750 && "$volume_encrypted" == true ]] \
    || fail "Preserved volume ${DATA_VOLUME_ID} is below the configured encrypted gp3 floor; review it before reuse."
  echo "Found preserved volume ${DATA_VOLUME_ID} in ${DATA_AZ}; it will be reused."
fi

# Confirm that the current multipart snapshot fits before any paid launch.
# Verified streaming import retains one part alongside the database; reserve
# 15% for filesystem overhead and growth. HEAD requests fetch metadata only.
SNAPSHOT_CAPACITY_GIB="$DATA_SIZE"
[[ "$REUSE_DATA" != true ]] || SNAPSHOT_CAPACITY_GIB="$volume_size"
python3 - "$SNAPSHOT_CAPACITY_GIB" <<'PY'
import concurrent.futures, re, subprocess, sys, urllib.parse
base = "https://snapshot.arbitrum.foundation/"
def curl(*args):
    return subprocess.check_output(
        ["curl", "--fail", "--silent", "--show-error", "--max-time", "30", *args],
        text=True)
pointer = curl(base + "arb1/latest-pruned.txt").strip()
snapshot = urllib.parse.urljoin(base, pointer)
if not snapshot.startswith(base + "arb1/"):
    raise SystemExit("Unexpected official snapshot URL; inspect before launch.")
try:
    manifest = curl(snapshot + ".manifest.txt")
except subprocess.CalledProcessError:
    raise SystemExit("Cannot verify multipart snapshot capacity; inspect the official snapshot before launch.")
parts = []
for line in manifest.splitlines():
    fields = line.split()
    if len(fields) != 2 or not re.fullmatch(r"[0-9a-f]{64}", fields[0]):
        raise SystemExit("Unexpected snapshot manifest format.")
    url = urllib.parse.urljoin(snapshot, "../" + fields[1])
    if not url.startswith(base + "arb1/"):
        raise SystemExit("Unexpected snapshot part URL.")
    parts.append(url)
if not parts:
    raise SystemExit("Snapshot manifest has no parts.")
def part_size(url):
    headers = curl("--head", url)
    sizes = re.findall(r"^content-length:\s*(\d+)", headers, re.I | re.M)
    if not sizes:
        raise SystemExit("Snapshot part has no verifiable Content-Length.")
    return int(sizes[-1])
with concurrent.futures.ThreadPoolExecutor(max_workers=5) as pool:
    sizes = list(pool.map(part_size, parts))
archive = sum(sizes)
peak = archive + max(sizes) + 4 * 2**30
capacity = int(sys.argv[1]) * 2**30
print(f"Snapshot: {snapshot}; archive {archive / 2**30:.2f} GiB; verified streaming peak {peak / 2**30:.2f} GiB")
if peak > capacity * .85:
    raise SystemExit("Snapshot streaming exceeds data-volume capacity with 15% reserve; review DATA_SIZE before launch.")
PY

if [[ -z "$ADMIN_CIDR" ]]; then
  admin_ip="$(curl --fail --silent --show-error --max-time 5 https://checkip.amazonaws.com 2>/dev/null | tr -d '[:space:]' || true)"
  if [[ "$admin_ip" =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]] && \
    python3 -c 'import ipaddress,sys; ipaddress.IPv4Address(sys.argv[1])' "$admin_ip" 2>/dev/null; then
    ADMIN_CIDR="${admin_ip}/32"
  fi
fi
if [[ -n "$ADMIN_CIDR" ]]; then
  python3 -c 'import ipaddress,sys; n=ipaddress.ip_network(sys.argv[1], strict=False); assert n.version == 4 and n.prefixlen == 32' "$ADMIN_CIDR" \
    || fail 'ADMIN_CIDR must be a single IPv4 address with a /32 prefix.'
fi

# Reuse the known Ohio key if present locally; otherwise create a dedicated key.
if [[ -z "${ARBITRUM_KEY_NAME:-}" && "$REGION" == us-east-2 && -f "${HOME}/.ssh/mev-ohio" ]] && \
  awsr ec2 describe-key-pairs --key-names mev-ohio >/dev/null 2>&1; then
  KEY_NAME=mev-ohio
  KEY_PATH="${HOME}/.ssh/mev-ohio"
fi
if [[ -f "$KEY_PATH" ]] && awsr ec2 describe-key-pairs --key-names "$KEY_NAME" >/dev/null 2>&1; then
  chmod 0600 "$KEY_PATH"
  echo "Reusing key pair ${KEY_NAME} at ${KEY_PATH}."
else
  key_json="$(awsr ec2 describe-key-pairs --key-names "$KEY_NAME" --output json 2>/dev/null || true)"
  if [[ -n "$key_json" && "$(jq -r '.KeyPairs[0].KeyPairId // empty' <<<"$key_json")" != '' ]]; then
    KEY_NAME="${KEY_NAME}-${REGION}"
    KEY_PATH="${HOME}/.ssh/${KEY_NAME}.pem"
  fi
  if ! awsr ec2 describe-key-pairs --key-names "$KEY_NAME" >/dev/null 2>&1; then
    install -d -m 0700 "$(dirname "$KEY_PATH")"
    umask 077
    awsr ec2 create-key-pair --key-name "$KEY_NAME" --key-type ed25519 \
      --tag-specifications 'ResourceType=key-pair,Tags=[{Key=Name,Value=arbitrum-one-node-key},{Key=Project,Value=LiquidationBot},{Key=Role,Value=NitroFullNode},{Key=Chain,Value=ArbitrumOne},{Key=ManagedBy,Value=aws-cli}]' \
      --query KeyMaterial --output text >"$KEY_PATH"
    chmod 0600 "$KEY_PATH"
    echo "Created key pair ${KEY_NAME}; private key stored mode 0600 at ${KEY_PATH}."
  else
    echo "Key pair ${KEY_NAME} has no local private key; SSH ingress will stay closed and SSM will be used."
    KEY_NAME=""
  fi
fi

# Select a current-generation x86_64 R-family type with at least 8 vCPU/64 GiB.
TYPE_META='{}'
SELECTED_TYPE=""
CANDIDATES=(r7i.2xlarge r8i.2xlarge r7a.2xlarge r7iz.2xlarge r6i.2xlarge)
for candidate in "${CANDIDATES[@]}"; do
  info="$(awsr ec2 describe-instance-types --instance-types "$candidate" --output json 2>/dev/null || true)"
  [[ -n "$info" ]] || continue
  if ! jq -e '.InstanceTypes[0] != null and
      (.InstanceTypes[0].ProcessorInfo.SupportedArchitectures | index("x86_64") != null) and
      .InstanceTypes[0].VCpuInfo.DefaultVCpus >= 8 and
      .InstanceTypes[0].MemoryInfo.SizeInMiB >= 65536 and
      .InstanceTypes[0].EbsInfo.EbsOptimizedSupport != "unsupported"' \
      >/dev/null <<<"$info"; then continue; fi
  offered="$(awsr ec2 describe-instance-type-offerings --location-type availability-zone \
    --filters "Name=instance-type,Values=${candidate}" --query 'InstanceTypeOfferings[].Location' --output json)"
  if [[ "$REUSE_DATA" == true ]]; then offered="$(jq --arg az "$DATA_AZ" '[.[] | select(. == $az)]' <<<"$offered")"; fi
  if [[ "$(jq 'length' <<<"$offered")" -gt 0 ]]; then
    SELECTED_TYPE="$candidate"
    type_item="$(jq -c '.InstanceTypes[0]' <<<"$info")"
    TYPE_META="$(jq --arg type "$candidate" --argjson info "$type_item" --argjson azs "$offered" \
      '. + {($type): {info:$info, azs:$azs}}' <<<"$TYPE_META")"
    break
  fi
done
[[ -n "$SELECTED_TYPE" ]] || fail 'No offered x86_64 R-family instance met 8 vCPU / 64 GiB.'

# R-family instances share the Standard On-Demand vCPU quota with the bot's
# T-family instance. Detect insufficient capacity before changing IAM/network.
while true; do
  quota_json="$(awsr service-quotas get-service-quota --service-code ec2 --quota-code L-1216C47A --output json)"
  active_json="$(awsr ec2 describe-instances --filters Name=instance-state-name,Values=pending,running,stopping --output json)"
  active_types=()
  readarray -t active_types < <(jq -r '[.Reservations[].Instances[].InstanceType | select(test("^[acdhimrtz]"))] | unique[]' <<<"$active_json")
  active_type_json='{"InstanceTypes":[]}'
  if (( ${#active_types[@]} > 0 )); then
    active_type_json="$(awsr ec2 describe-instance-types --instance-types "${active_types[@]}" --output json)"
  fi
  required_vcpus="$(jq -r --arg type "$SELECTED_TYPE" '.[$type].info.VCpuInfo.DefaultVCpus' <<<"$TYPE_META")"
  used_vcpus="$(jq -n --argjson instances "$active_json" --argjson types "$active_type_json" '
    ($types.InstanceTypes | map({key:.InstanceType,value:.VCpuInfo.DefaultVCpus}) | from_entries) as $cpu |
    [$instances.Reservations[].Instances[] | select(.InstanceType | test("^[acdhimrtz]")) |
      (if .CpuOptions.CoreCount != null and .CpuOptions.ThreadsPerCore != null
        then .CpuOptions.CoreCount * .CpuOptions.ThreadsPerCore else $cpu[.InstanceType] end)] | add // 0')"
  quota_value="$(jq -r '.Quota.Value' <<<"$quota_json")"
  echo "Standard On-Demand vCPU quota: limit=${quota_value}, in use=${used_vcpus}, new node=${required_vcpus}."
  if awk -v used="$used_vcpus" -v required="$required_vcpus" -v limit="$quota_value" 'BEGIN {exit !(used+required <= limit)}'; then
    break
  fi
  [[ "$WAIT_FOR_QUOTA" == true ]] \
    || fail "Insufficient Standard On-Demand vCPU quota in ${REGION}; wait for the requested quota increase. No instance was launched."
  echo "$(date -Is): Waiting for AWS quota approval; existing workloads remain running."
  sleep 60
done

# Current Canonical Ubuntu 24.04 amd64 AMI via its public SSM parameter.
AMI_PARAM=/aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id
AMI_ID="$(awsr ssm get-parameter --name "$AMI_PARAM" --query 'Parameter.Value' --output text)"
AMI_JSON="$(awsr ec2 describe-images --image-ids "$AMI_ID" --output json)"
AMI_ARCH="$(jq -r '.Images[0].Architecture // ""' <<<"$AMI_JSON")"
AMI_OWNER="$(jq -r '.Images[0].OwnerId // ""' <<<"$AMI_JSON")"
AMI_STATE="$(jq -r '.Images[0].State // ""' <<<"$AMI_JSON")"
ROOT_DEVICE="$(jq -r '.Images[0].RootDeviceName // ""' <<<"$AMI_JSON")"
[[ "$AMI_ARCH" == x86_64 && "$AMI_OWNER" == 099720109477 && "$AMI_STATE" == available ]] \
  || fail "Ubuntu AMI validation failed (${AMI_ARCH}/${AMI_OWNER}/${AMI_STATE})."

# Find a subnet with an active Internet route and instance-type support. Public
# subnets are preferred; a NAT subnet is accepted if no public one is suitable.
vpcs="$(awsr ec2 describe-vpcs --output json)"
VPC_ID=""
SUBNET_ID=""
AZ=""
NETWORK_MODE=""
try_vpc() {
  local vpc="$1" subnets routes subnet az route_id route public private
  subnets="$(awsr ec2 describe-subnets --filters "Name=vpc-id,Values=${vpc}" --output json)"
  routes="$(awsr ec2 describe-route-tables --filters "Name=vpc-id,Values=${vpc}" --output json)"
  while IFS=$'\t' read -r subnet az; do
    [[ -n "$subnet" && -n "$az" ]] || continue
    [[ "$REUSE_DATA" != true || "$az" == "$DATA_AZ" ]] || continue
    jq -e --arg az "$az" --arg type "$SELECTED_TYPE" '.[$type].azs | index($az) != null' >/dev/null <<<"$TYPE_META" || continue
    route_id="$(jq -r --arg subnet "$subnet" '.RouteTables[] | select(any(.Associations[]?; .SubnetId == $subnet)) | .RouteTableId' <<<"$routes" | head -n1)"
    if [[ -z "$route_id" ]]; then route_id="$(jq -r '.RouteTables[] | select(any(.Associations[]?; .Main == true)) | .RouteTableId' <<<"$routes" | head -n1)"; fi
    [[ -n "$route_id" ]] || continue
    route="$(jq -c --arg id "$route_id" '.RouteTables[] | select(.RouteTableId == $id) | .Routes' <<<"$routes")"
    public="$(jq -r 'any(.[]; .DestinationCidrBlock == "0.0.0.0/0" and ((.GatewayId // "") | startswith("igw-")) and .State == "active")' <<<"$route")"
    private="$(jq -r 'any(.[]; .DestinationCidrBlock == "0.0.0.0/0" and ((.NatGatewayId // "") | startswith("nat-")) and .State == "active")' <<<"$route")"
    if [[ "$public" == true ]]; then NETWORK_MODE=public; elif [[ "$private" == true ]]; then NETWORK_MODE=private; else continue; fi
    VPC_ID="$vpc"; SUBNET_ID="$subnet"; AZ="$az"
    [[ "$NETWORK_MODE" != public ]] || return 0
  done < <(jq -r '.Subnets | sort_by(.AvailabilityZone)[] | [.SubnetId,.AvailabilityZone] | @tsv' <<<"$subnets")
  [[ -n "$SUBNET_ID" ]]
}
while read -r vpc; do
  [[ -n "$vpc" ]] || continue
  if try_vpc "$vpc" && [[ "$NETWORK_MODE" == public ]]; then break; fi
done < <(jq -r '.Vpcs | sort_by(if .IsDefault then 0 else 1 end) | .[].VpcId' <<<"$vpcs")
if [[ -z "$SUBNET_ID" ]]; then
  while read -r vpc; do [[ -n "$vpc" ]] || continue; if try_vpc "$vpc"; then break; fi; done < <(jq -r '.Vpcs[].VpcId' <<<"$vpcs")
fi

# No suitable existing network: create a minimal tagged public VPC in the
# selected type's first supported AZ, with outbound Internet access.
if [[ -z "$SUBNET_ID" ]]; then
  VPC_ID="$(jq -r '.Vpcs[] | select(any(.Tags[]?; .Key == "Name" and .Value == "arbitrum-one-vpc")) | .VpcId' <<<"$vpcs" | head -n1)"
  if [[ -z "$VPC_ID" ]]; then
    VPC_ID="$(awsr ec2 create-vpc --cidr-block 10.72.0.0/16 \
      --tag-specifications 'ResourceType=vpc,Tags=[{Key=Name,Value=arbitrum-one-vpc},{Key=Project,Value=LiquidationBot},{Key=Role,Value=NitroFullNode},{Key=Chain,Value=ArbitrumOne},{Key=ManagedBy,Value=aws-cli}]' \
      --query Vpc.VpcId --output text)"
  fi
  awsr ec2 modify-vpc-attribute --vpc-id "$VPC_ID" --enable-dns-support '{"Value":true}'
  awsr ec2 modify-vpc-attribute --vpc-id "$VPC_ID" --enable-dns-hostnames '{"Value":true}'
  awsr ec2 wait vpc-available --vpc-ids "$VPC_ID"
  AZ="$(jq -r --arg type "$SELECTED_TYPE" '.[$type].azs[0] // empty' <<<"$TYPE_META")"
  [[ -n "$AZ" ]] || fail "No AZ offers ${SELECTED_TYPE}."
  SUBNET_ID="$(awsr ec2 describe-subnets --filters "Name=vpc-id,Values=${VPC_ID}" "Name=availability-zone,Values=${AZ}" \
    --query 'Subnets[0].SubnetId' --output text)"
  if [[ -z "$SUBNET_ID" || "$SUBNET_ID" == None ]]; then
    SUBNET_ID="$(awsr ec2 create-subnet --vpc-id "$VPC_ID" --cidr-block 10.72.1.0/24 --availability-zone "$AZ" \
      --tag-specifications 'ResourceType=subnet,Tags=[{Key=Name,Value=arbitrum-one-public-subnet},{Key=Project,Value=LiquidationBot},{Key=Role,Value=NitroFullNode},{Key=Chain,Value=ArbitrumOne},{Key=ManagedBy,Value=aws-cli}]' \
      --query Subnet.SubnetId --output text)"
  fi
  awsr ec2 modify-subnet-attribute --subnet-id "$SUBNET_ID" --map-public-ip-on-launch
  igw="$(awsr ec2 describe-internet-gateways --filters "Name=attachment.vpc-id,Values=${VPC_ID}" --query 'InternetGateways[0].InternetGatewayId' --output text)"
  if [[ -z "$igw" || "$igw" == None ]]; then
    igw="$(awsr ec2 create-internet-gateway --tag-specifications 'ResourceType=internet-gateway,Tags=[{Key=Name,Value=arbitrum-one-igw},{Key=Project,Value=LiquidationBot},{Key=Role,Value=NitroFullNode},{Key=Chain,Value=ArbitrumOne},{Key=ManagedBy,Value=aws-cli}]' --query InternetGateway.InternetGatewayId --output text)"
    awsr ec2 attach-internet-gateway --internet-gateway-id "$igw" --vpc-id "$VPC_ID"
  fi
  rt="$(awsr ec2 describe-route-tables --filters "Name=vpc-id,Values=${VPC_ID}" Name=tag:Name,Values=arbitrum-one-public-routes Name=tag:Project,Values=LiquidationBot --query 'RouteTables[0].RouteTableId' --output text)"
  if [[ -z "$rt" || "$rt" == None ]]; then
    rt="$(awsr ec2 create-route-table --vpc-id "$VPC_ID" --tag-specifications 'ResourceType=route-table,Tags=[{Key=Name,Value=arbitrum-one-public-routes},{Key=Project,Value=LiquidationBot},{Key=Role,Value=NitroFullNode},{Key=Chain,Value=ArbitrumOne},{Key=ManagedBy,Value=aws-cli}]' --query RouteTable.RouteTableId --output text)"
  fi
  route_info="$(awsr ec2 describe-route-tables --route-table-ids "$rt" --output json)"
  if ! jq -e --arg igw "$igw" 'any(.RouteTables[0].Routes[]; .DestinationCidrBlock == "0.0.0.0/0" and .GatewayId == $igw and .State == "active")' >/dev/null <<<"$route_info"; then
    awsr ec2 create-route --route-table-id "$rt" --destination-cidr-block 0.0.0.0/0 --gateway-id "$igw"
  fi
  if ! jq -e --arg subnet "$SUBNET_ID" 'any(.RouteTables[0].Associations[]?; .SubnetId == $subnet)' >/dev/null <<<"$route_info"; then
    awsr ec2 associate-route-table --route-table-id "$rt" --subnet-id "$SUBNET_ID"
  fi
  NETWORK_MODE=public
fi

# Dedicated SG: no public RPC/WS/feed ports. Replace ingress only on a tagged
# group this script owns, then add the discovered admin /32 if available.
sg_json="$(awsr ec2 describe-security-groups --filters "Name=vpc-id,Values=${VPC_ID}" "Name=group-name,Values=${SG_NAME}" --output json)"
SG_ID="$(jq -r '.SecurityGroups[0].GroupId // empty' <<<"$sg_json")"
if [[ -n "$SG_ID" ]] && ! jq -e '(.SecurityGroups[0].Tags // []) as $t | any($t[]; .Key == "ManagedBy" and .Value == "aws-cli") and any($t[]; .Key == "Project" and .Value == "LiquidationBot")' >/dev/null <<<"$sg_json"; then
  fail "Security group ${SG_NAME} exists but is not owned by this deployment."
fi
if [[ -z "$SG_ID" ]]; then
  SG_ID="$(awsr ec2 create-security-group --group-name "$SG_NAME" --description 'Arbitrum Nitro; RPC restricted to loopback' --vpc-id "$VPC_ID" \
    --tag-specifications 'ResourceType=security-group,Tags=[{Key=Name,Value=arbitrum-one-node-sg},{Key=Project,Value=LiquidationBot},{Key=Role,Value=NitroFullNode},{Key=Chain,Value=ArbitrumOne},{Key=ManagedBy,Value=aws-cli}]' --query GroupId --output text)"
fi
sg_rules="$(awsr ec2 describe-security-groups --group-ids "$SG_ID" --query 'SecurityGroups[0].IpPermissions' --output json)"
if [[ "$sg_rules" != '[]' && "$sg_rules" != null ]]; then
  rule_file="$(mktemp)"; printf '%s\n' "$sg_rules" >"$rule_file"
  awsr ec2 revoke-security-group-ingress --group-id "$SG_ID" --ip-permissions "file://${rule_file}"
  rm -f -- "$rule_file"
fi
egress="$(awsr ec2 describe-security-groups --group-ids "$SG_ID" --query 'SecurityGroups[0].IpPermissionsEgress' --output json)"
if ! jq -e 'any(.[]; .IpProtocol == "-1" and any(.IpRanges[]?; .CidrIp == "0.0.0.0/0"))' >/dev/null <<<"$egress"; then
  awsr ec2 authorize-security-group-egress --group-id "$SG_ID" --ip-permissions '[{"IpProtocol":"-1","IpRanges":[{"CidrIp":"0.0.0.0/0"}]}]'
fi
if [[ "$NETWORK_MODE" == public && -n "$ADMIN_CIDR" && -n "$KEY_NAME" && -f "$KEY_PATH" ]]; then
  awsr ec2 authorize-security-group-ingress --group-id "$SG_ID" --protocol tcp --port 22 --cidr "$ADMIN_CIDR"
else echo 'SSH ingress stays closed; use SSM Session Manager.'; fi

# EC2 role has SSM core access and only reads the two parent-RPC parameters.
temp_dir="$(mktemp -d)"
chmod 0700 "$temp_dir"
trap 'rm -rf -- "$temp_dir"' EXIT
jq -n '{Version:"2012-10-17",Statement:[{Effect:"Allow",Principal:{Service:"ec2.amazonaws.com"},Action:"sts:AssumeRole"}]}' >"$temp_dir/trust.json"
role_json="$(awsiam iam get-role --role-name "$ROLE_NAME" --output json 2>/dev/null || true)"
if [[ -z "$role_json" ]]; then
  awsiam iam create-role --role-name "$ROLE_NAME" --assume-role-policy-document "file://${temp_dir}/trust.json" \
    --description 'Arbitrum Nitro SSM access and scoped parent RPC reads' \
    --tags Key=Name,Value=arbitrum-one-node-ssm-role Key=Project,Value=LiquidationBot Key=Role,Value=NitroFullNode Key=Chain,Value=ArbitrumOne Key=ManagedBy,Value=aws-cli
else
  jq -e '(.Role.Tags // []) as $t | any($t[]; .Key == "ManagedBy" and .Value == "aws-cli") and any($t[]; .Key == "Project" and .Value == "LiquidationBot")' >/dev/null <<<"$role_json" || fail "IAM role ${ROLE_NAME} is not deployment-owned."
fi
attached="$(awsiam iam list-attached-role-policies --role-name "$ROLE_NAME" --query 'AttachedPolicies[].PolicyArn' --output text)"
if [[ " $attached " != *' arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore '* ]]; then
  awsiam iam attach-role-policy --role-name "$ROLE_NAME" --policy-arn arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore
fi
jq -n --arg region "$REGION" --arg account "$ACCOUNT_ID" '{Version:"2012-10-17",Statement:[{Sid:"ReadParentRpcSecureStrings",Effect:"Allow",Action:["ssm:GetParameter"],Resource:("arn:aws:ssm:"+$region+":"+$account+":parameter/mev-bot/arbitrum/*")},{Sid:"DecryptOnlyThroughSSM",Effect:"Allow",Action:["kms:Decrypt"],Resource:"*",Condition:{StringEquals:{"kms:ViaService":("ssm."+$region+".amazonaws.com")},StringLike:{"kms:EncryptionContext:PARAMETER_ARN":("arn:aws:ssm:"+$region+":"+$account+":parameter/mev-bot/arbitrum/*")}}}]}' >"$temp_dir/read-parent.json"
awsiam iam put-role-policy --role-name "$ROLE_NAME" --policy-name ArbitrumParentRpcParameterRead --policy-document "file://${temp_dir}/read-parent.json"
# AmazonSSMManagedInstanceCore also allows GetParameter(s) on all resources.
# An explicit deny is needed to keep the node's parameter reads scoped.
jq -n --arg region "$REGION" --arg account "$ACCOUNT_ID" '{Version:"2012-10-17",Statement:[{Sid:"DenyOtherParameterReads",Effect:"Deny",Action:["ssm:GetParameter","ssm:GetParameters","ssm:GetParametersByPath"],NotResource:("arn:aws:ssm:"+$region+":"+$account+":parameter/mev-bot/arbitrum/*")}]}' >"$temp_dir/parameter-boundary.json"
awsiam iam put-role-policy --role-name "$ROLE_NAME" --policy-name ArbitrumParameterReadBoundary --policy-document "file://${temp_dir}/parameter-boundary.json"
profile_json="$(awsiam iam get-instance-profile --instance-profile-name "$PROFILE_NAME" --output json 2>/dev/null || true)"
if [[ -z "$profile_json" ]]; then
  awsiam iam create-instance-profile --instance-profile-name "$PROFILE_NAME" \
    --tags Key=Name,Value=arbitrum-one-node-ssm-profile Key=Project,Value=LiquidationBot Key=Role,Value=NitroFullNode Key=Chain,Value=ArbitrumOne Key=ManagedBy,Value=aws-cli
else
  jq -e '(.InstanceProfile.Tags // []) as $t | any($t[]; .Key == "ManagedBy" and .Value == "aws-cli") and any($t[]; .Key == "Project" and .Value == "LiquidationBot")' >/dev/null <<<"$profile_json" || fail "IAM instance profile ${PROFILE_NAME} is not deployment-owned."
fi
profile_roles="$(awsiam iam get-instance-profile --instance-profile-name "$PROFILE_NAME" --query 'InstanceProfile.Roles[].RoleName' --output text)"
if [[ -n "$profile_roles" && "$profile_roles" != None && "$profile_roles" != "$ROLE_NAME" ]]; then fail "Instance profile already contains ${profile_roles}."; fi
if [[ " $profile_roles " != *" ${ROLE_NAME} "* ]]; then awsiam iam add-role-to-instance-profile --instance-profile-name "$PROFILE_NAME" --role-name "$ROLE_NAME"; fi
sleep 10

# Store caller-supplied endpoints as SecureStrings via a 0600 JSON temp file;
# values never appear in user data, terminal output, or command arguments.
store_parent_endpoints

# Rough current monthly estimate; region-level differences are small. The EC2
# action occurs only after this exact resource plan is printed.
INSTANCE_HOURLY="${ARBITRUM_COST_EC2_HOURLY:-0.5292}"
if [[ "$REUSE_DATA" == true ]]; then
  DATA_STORAGE_MONTHLY="$(awk -v size="$volume_size" 'BEGIN {printf "%.2f",size*0.08}')"
  DATA_IOPS_MONTHLY="$(awk -v iops="$volume_iops" 'BEGIN {printf "%.2f",(iops-3000)*0.005}')"
  DATA_THROUGHPUT_MONTHLY="$(awk -v t="$volume_throughput" 'BEGIN {printf "%.2f",(t-125)*0.04}')"
else
  DATA_STORAGE_MONTHLY="$(awk -v size="$DATA_SIZE" 'BEGIN {printf "%.2f",size*0.08}')"
  DATA_IOPS_MONTHLY="$(awk -v iops="$DATA_IOPS" 'BEGIN {printf "%.2f",(iops-3000)*0.005}')"
  DATA_THROUGHPUT_MONTHLY="$(awk -v t="$DATA_THROUGHPUT" 'BEGIN {printf "%.2f",(t-125)*0.04}')"
fi
ROOT_MONTHLY=4.00
PUBLIC_IP_MONTHLY=0.00
[[ "$NETWORK_MODE" != public ]] || PUBLIC_IP_MONTHLY=3.65
ESTIMATE="$(awk -v i="$INSTANCE_HOURLY" -v d="$DATA_STORAGE_MONTHLY" -v p="$DATA_IOPS_MONTHLY" -v t="$DATA_THROUGHPUT_MONTHLY" -v r="$ROOT_MONTHLY" -v ip="$PUBLIC_IP_MONTHLY" 'BEGIN{printf "%.2f",i*730+d+p+t+r+ip}')"
echo
echo 'PRE-LAUNCH RESOURCE / COST PLAN'
echo "  EC2: ${SELECTED_TYPE}, On-Demand Linux, ${REGION}/${AZ}, x86_64"
echo "  Ubuntu 24.04 Canonical AMI: ${AMI_ID}"
echo "  Root: ${ROOT_SIZE} GiB encrypted gp3, delete on termination"
if [[ "$REUSE_DATA" == true ]]; then
  echo "  Data: reuse ${DATA_VOLUME_ID}, ${volume_size} GiB encrypted gp3, ${volume_iops} IOPS, ${volume_throughput} MiB/s, preserve on termination"
else echo "  Data: new ${DATA_SIZE} GiB encrypted gp3, ${DATA_IOPS} IOPS, ${DATA_THROUGHPUT} MiB/s, preserve on termination"; fi
echo "  Network: ${VPC_ID}/${SUBNET_ID} (${NETWORK_MODE}); SSH ${ADMIN_CIDR:-closed}; public RPC/WS/feed closed"
echo "  Nitro image: ${NITRO_IMAGE} (linux/amd64)"
echo "  Estimated 730-hour monthly subtotal: \$${ESTIMATE} (EC2, gp3, public IPv4 if assigned)"
echo '  Excludes transfer, snapshots/backups, tax, and unrelated account usage.'
echo

# Persist only non-secret configuration in a gzip-compressed cloud-init bundle.
tar -cJf "$temp_dir/arbitrum-bootstrap.tar.xz" -C "$SCRIPT_DIR" \
  bootstrap-arbitrum-node.sh nitro-config.json nitro.env.example nitro-node.service nitro-start.sh arbitrum-status arbitrum-snapshot-init.py arbitrum-snapshot-init.service arbitrum-parent-ready.py arbitrum-activate.sh arbitrum-activate.service arbitrum-activate.timer arbitrum-health.service arbitrum-health.timer
bundle="$(python3 - "$temp_dir/arbitrum-bootstrap.tar.xz" <<'PY'
import base64, pathlib, sys
value = base64.b85encode(pathlib.Path(sys.argv[1]).read_bytes()).decode()
assert "'" not in value and "\n" not in value
print(value)
PY
)"
cat >"$temp_dir/user-data.sh" <<EOF
#!/bin/bash
set -Eeuo pipefail
install -d -m 0755 /opt/arbitrum-node-bootstrap
printf '%s' '${bundle}' | python3 -c 'import base64,lzma,sys; sys.stdout.buffer.write(lzma.decompress(base64.b85decode(sys.stdin.buffer.read())))' | tar -x -C /opt/arbitrum-node-bootstrap --no-same-owner
exec /bin/bash /opt/arbitrum-node-bootstrap/bootstrap-arbitrum-node.sh
EOF
chmod 0600 "$temp_dir/user-data.sh"
userdata_size="$(wc -c <"$temp_dir/user-data.sh")"
(( userdata_size <= 16384 )) || fail "User data is ${userdata_size} bytes; EC2 limit is 16 KiB."

jq -n --arg root "$ROOT_DEVICE" --argjson root_size "$ROOT_SIZE" --argjson data_size "$DATA_SIZE" \
  --argjson iops "$DATA_IOPS" --argjson throughput "$DATA_THROUGHPUT" --argjson reuse "$REUSE_DATA" \
  '[{DeviceName:$root,Ebs:{VolumeSize:$root_size,VolumeType:"gp3",Encrypted:true,DeleteOnTermination:true}}] +
   (if $reuse then [] else [{DeviceName:"/dev/sdf",Ebs:{VolumeSize:$data_size,VolumeType:"gp3",Iops:$iops,Throughput:$throughput,Encrypted:true,DeleteOnTermination:false}}] end)' \
  >"$temp_dir/block-device-mappings.json"
jq -n '[{ResourceType:"instance",Tags:[{Key:"Name",Value:"arbitrum-one-node"},{Key:"Project",Value:"LiquidationBot"},{Key:"Role",Value:"NitroFullNode"},{Key:"Chain",Value:"ArbitrumOne"},{Key:"ManagedBy",Value:"aws-cli"}]},{ResourceType:"volume",Tags:[{Key:"Name",Value:"arbitrum-one-node-data"},{Key:"Project",Value:"LiquidationBot"},{Key:"Role",Value:"NitroFullNode"},{Key:"Chain",Value:"ArbitrumOne"},{Key:"ManagedBy",Value:"aws-cli"}]}]' >"$temp_dir/tags.json"
network_args=(--subnet-id "$SUBNET_ID" --security-group-ids "$SG_ID")
[[ "$NETWORK_MODE" != public ]] || network_args+=(--associate-public-ip-address)
run_args=(--image-id "$AMI_ID" --instance-type "$SELECTED_TYPE" --count 1 \
  --block-device-mappings "file://${temp_dir}/block-device-mappings.json" \
  --tag-specifications "file://${temp_dir}/tags.json" \
  --iam-instance-profile "Name=${PROFILE_NAME}" \
  --metadata-options 'HttpTokens=required,HttpEndpoint=enabled,HttpPutResponseHopLimit=2' \
  --user-data "file://${temp_dir}/user-data.sh" --instance-initiated-shutdown-behavior stop)
[[ -z "$KEY_NAME" ]] || run_args+=(--key-name "$KEY_NAME")
awsr ec2 run-instances "${run_args[@]}" "${network_args[@]}" >"$temp_dir/run.json"
INSTANCE_ID="$(jq -r '.Instances[0].InstanceId // empty' "$temp_dir/run.json")"
[[ -n "$INSTANCE_ID" ]] || fail 'run-instances returned no InstanceId.'
printf 'INSTANCE_ID=%s\nINSTANCE_TYPE=%s\nREGION=%s\nAZ=%s\nDATA_VOLUME_ID=%s\nSECURITY_GROUP_ID=%s\n' \
  "$INSTANCE_ID" "$SELECTED_TYPE" "$REGION" "$AZ" "$DATA_VOLUME_ID" "$SG_ID" >"$STATE_FILE"
chmod 0600 "$STATE_FILE"
if [[ "$REUSE_DATA" == true ]]; then
  awsr ec2 attach-volume --volume-id "$DATA_VOLUME_ID" --instance-id "$INSTANCE_ID" --device /dev/sdf >/dev/null
fi
awsr ec2 wait instance-running --instance-ids "$INSTANCE_ID"
awsr ec2 wait instance-status-ok --instance-ids "$INSTANCE_ID"
instance="$(awsr ec2 describe-instances --instance-ids "$INSTANCE_ID" --output json)"
PUBLIC_IP="$(jq -r '.Reservations[0].Instances[0].PublicIpAddress // ""' <<<"$instance")"
PRIVATE_IP="$(jq -r '.Reservations[0].Instances[0].PrivateIpAddress // ""' <<<"$instance")"
ROOT_VOLUME_ID="$(jq -r --arg root "$ROOT_DEVICE" '.Reservations[0].Instances[0].BlockDeviceMappings[]? | select(.DeviceName == $root) | .Ebs.VolumeId' <<<"$instance" | head -n1)"
ATTACHED_DATA_ID="$(jq -r '.Reservations[0].Instances[0].BlockDeviceMappings[]? | select(.DeviceName == "/dev/sdf") | .Ebs.VolumeId' <<<"$instance" | head -n1)"
[[ -n "$ATTACHED_DATA_ID" && "$ATTACHED_DATA_ID" != null ]] || fail "Data volume missing from instance ${INSTANCE_ID}; instance retained for recovery."
DATA_VOLUME_ID="$ATTACHED_DATA_ID"
awsr ec2 create-tags --resources "$INSTANCE_ID" "$ROOT_VOLUME_ID" \
  --tags Key=Name,Value=arbitrum-one-node Key=Project,Value=LiquidationBot Key=Role,Value=NitroFullNode Key=Chain,Value=ArbitrumOne Key=ManagedBy,Value=aws-cli
awsr ec2 create-tags --resources "$DATA_VOLUME_ID" \
  --tags Key=Name,Value=arbitrum-one-node-data Key=Project,Value=LiquidationBot Key=Role,Value=NitroFullNode Key=Chain,Value=ArbitrumOne Key=ManagedBy,Value=aws-cli
printf 'INSTANCE_ID=%s\nINSTANCE_TYPE=%s\nREGION=%s\nAZ=%s\nPUBLIC_IP=%s\nPRIVATE_IP=%s\nDATA_VOLUME_ID=%s\nSECURITY_GROUP_ID=%s\nVPC_ID=%s\nSUBNET_ID=%s\nROOT_VOLUME_ID=%s\nNITRO_IMAGE=%s\n' \
  "$INSTANCE_ID" "$SELECTED_TYPE" "$REGION" "$AZ" "$PUBLIC_IP" "$PRIVATE_IP" "$DATA_VOLUME_ID" "$SG_ID" "$VPC_ID" "$SUBNET_ID" "$ROOT_VOLUME_ID" "$NITRO_IMAGE" >"$STATE_FILE"
chmod 0600 "$STATE_FILE"
echo 'EC2 status checks are healthy. Waiting for SSM agent/bootstrap log availability.'
SSM_READY=false
for _ in $(seq 1 48); do
  ssm_id="$(awsr ssm describe-instance-information --filters "Key=InstanceIds,Values=${INSTANCE_ID}" --query 'InstanceInformationList[0].InstanceId' --output text 2>/dev/null || true)"
  if [[ "$ssm_id" == "$INSTANCE_ID" ]]; then SSM_READY=true; break; fi
  sleep 5
done
if [[ "$SSM_READY" == true ]]; then
  echo 'SSM is online; bootstrap log tail:'
  cmd_id="$(awsr ssm send-command --instance-ids "$INSTANCE_ID" --document-name AWS-RunShellScript \
    --parameters 'commands=["tail -n 30 /var/log/arbitrum-bootstrap.log 2>/dev/null || tail -n 30 /var/log/cloud-init-output.log"]' \
    --query 'Command.CommandId' --output text)"
  for _ in $(seq 1 24); do
    result="$(awsr ssm get-command-invocation --command-id "$cmd_id" --instance-id "$INSTANCE_ID" --output json 2>/dev/null || true)"
    status="$(jq -r '.Status // "Pending"' <<<"$result" 2>/dev/null || echo Pending)"
    if [[ "$status" == Success || "$status" == Failed || "$status" == Cancelled || "$status" == TimedOut ]]; then
      jq -r '.StandardOutputContent // .StandardErrorContent // empty' <<<"$result"
      break
    fi
    sleep 5
  done
else echo 'SSM is still coming online; cloud-init will continue automatically.'; fi

echo
echo "INSTANCE_ID=${INSTANCE_ID}"
echo "INSTANCE_TYPE=${SELECTED_TYPE}"
echo "REGION=${REGION}"
echo "AZ=${AZ}"
echo "PUBLIC_IP=${PUBLIC_IP}"
echo "PRIVATE_IP=${PRIVATE_IP}"
echo "DATA_VOLUME_ID=${DATA_VOLUME_ID}"
echo 'DATA_MOUNT=/data/arbitrum'
echo "SECURITY_GROUP_ID=${SG_ID}"
if [[ "$NETWORK_MODE" == public && -n "$ADMIN_CIDR" && -n "$KEY_NAME" && -f "$KEY_PATH" ]]; then
  echo "SSH_COMMAND=ssh -i ${KEY_PATH} -o IdentitiesOnly=yes ubuntu@${PUBLIC_IP}"
else echo "SSH_COMMAND=aws --profile ${PROFILE} --region ${REGION} ssm start-session --target ${INSTANCE_ID}"; fi
echo 'RPC_HTTP=http://127.0.0.1:8547'
echo 'RPC_WS=ws://127.0.0.1:8548'
echo "RESOURCE_STATE=${STATE_FILE}"
echo 'Nitro can prepare the snapshot without parent endpoints; live RPC/catch-up requires both Ethereum URLs.'
