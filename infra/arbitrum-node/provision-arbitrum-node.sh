#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROFILE="${AWS_PROFILE:-mev-bot}"
REGION="${AWS_REGION:-${AWS_DEFAULT_REGION:-}}"
ADMIN_CIDR="${ADMIN_CIDR:-}"
KEY_NAME="${ARBITRUM_KEY_NAME:-arbitrum-one-node-key}"
KEY_PATH="${ARBITRUM_KEY_PATH:-/tmp/arbitrum-one-node-key.pem}"
STATE_FILE="${ARBITRUM_STATE_FILE:-${SCRIPT_DIR}/arbitrum-node-resources.env}"
INSTANCE_NAME="arbitrum-one-node"
DATA_VOLUME_NAME="arbitrum-one-node-data"
SG_NAME="arbitrum-one-node-sg"
ROLE_NAME="arbitrum-one-node-ssm-role"
PROFILE_NAME="arbitrum-one-node-ssm-profile"
DATA_SIZE=4096
DATA_IOPS=16000
DATA_THROUGHPUT=1000
ROOT_SIZE=50
NITRO_IMAGE="offchainlabs/nitro-node:v3.12.1-70fa99a"
AWS_PAGER=""
export AWS_PAGER

usage() {
  cat <<'EOF'
Usage: provision-arbitrum-node.sh [--region REGION] [--profile PROFILE] [--admin-cidr IPv4/32]

Uses existing suitable networking where possible, otherwise creates a small
tagged public VPC. AWS_PROFILE/AWS_REGION, ADMIN_CIDR, ARBITRUM_KEY_NAME,
ARBITRUM_KEY_PATH, ARBITRUM_STATE_FILE, and (optionally) ETHEREUM_RPC_URL /
ETHEREUM_BEACON_URL can also be set in the environment. Existing SSM secrets
are preserved unless ARBITRUM_OVERWRITE_SSM_PARAMETERS=1 is explicitly set.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --region) REGION="$2"; shift 2 ;;
    --profile) PROFILE="$2"; shift 2 ;;
    --admin-cidr) ADMIN_CIDR="$2"; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "$REGION" ]]; then
  REGION="$(aws configure get region --profile "$PROFILE" 2>/dev/null || true)"
fi
if [[ -z "$REGION" ]]; then
  REGION=us-east-1
fi

awsr() { aws --profile "$PROFILE" --region "$REGION" "$@"; }
awsiam() { aws --profile "$PROFILE" "$@"; }
fail() { echo "ERROR: $*" >&2; exit 1; }

command -v aws >/dev/null || fail "AWS CLI v2 is required."
command -v jq >/dev/null || fail "jq is required for safe AWS response parsing."
identity="$(awsiam sts get-caller-identity --output json)"
ACCOUNT_ID="$(jq -r '.Account' <<<"$identity")"
CALLER_ARN="$(jq -r '.Arn' <<<"$identity")"
echo "AWS account: ${ACCOUNT_ID} (${CALLER_ARN})"
echo "Selected region: ${REGION}"

# Reuse an existing matching instance; never make a duplicate expensive node.
existing="$(awsr ec2 describe-instances \
  --filters "Name=tag:Name,Values=${INSTANCE_NAME}" \
    Name=tag:Project,Values=LiquidationBot Name=tag:Role,Values=NitroFullNode \
    Name=instance-state-name,Values=pending,running,stopping,stopped \
  --output json)"
existing_ids="$(jq -r '.Reservations[].Instances[].InstanceId' <<<"$existing")"
if [[ -n "$existing_ids" ]]; then
  readarray -t ids <<<"$existing_ids"
  [[ ${#ids[@]} -eq 1 ]] || fail "Found multiple active instances tagged Name=${INSTANCE_NAME}; refusing to choose one."
  INSTANCE_ID="${ids[0]}"
  details="$(awsr ec2 describe-instances --instance-ids "$INSTANCE_ID" --output json)"
  INSTANCE_TYPE="$(jq -r '.Reservations[0].Instances[0].InstanceType' <<<"$details")"
  AZ="$(jq -r '.Reservations[0].Instances[0].Placement.AvailabilityZone' <<<"$details")"
  PUBLIC_IP="$(jq -r '.Reservations[0].Instances[0].PublicIpAddress // ""' <<<"$details")"
  PRIVATE_IP="$(jq -r '.Reservations[0].Instances[0].PrivateIpAddress // ""' <<<"$details")"
  SG_ID="$(jq -r '.Reservations[0].Instances[0].SecurityGroups[0].GroupId // ""' <<<"$details")"
  DATA_VOLUME_ID="$(jq -r '.Reservations[0].Instances[0].BlockDeviceMappings[]? | select(.DeviceName == "/dev/sdf") | .Ebs.VolumeId' <<<"$details" | head -n1)"
  INSTANCE_STATE="$(jq -r '.Reservations[0].Instances[0].State.Name' <<<"$details")"
  cat >"$STATE_FILE" <<EOF
INSTANCE_ID=${INSTANCE_ID}
INSTANCE_TYPE=${INSTANCE_TYPE}
REGION=${REGION}
AZ=${AZ}
PUBLIC_IP=${PUBLIC_IP}
PRIVATE_IP=${PRIVATE_IP}
DATA_VOLUME_ID=${DATA_VOLUME_ID}
SECURITY_GROUP_ID=${SG_ID}
EOF
  chmod 0600 "$STATE_FILE"
  echo "Reusing tagged instance ${INSTANCE_ID} (${INSTANCE_STATE}); no new instance or volume created."
  echo "INSTANCE_ID=${INSTANCE_ID}"
  echo "INSTANCE_TYPE=${INSTANCE_TYPE}"
  echo "REGION=${REGION}"
  echo "AZ=${AZ}"
  echo "PUBLIC_IP=${PUBLIC_IP}"
  echo "PRIVATE_IP=${PRIVATE_IP}"
  echo "DATA_VOLUME_ID=${DATA_VOLUME_ID}"
  echo "SECURITY_GROUP_ID=${SG_ID}"
  exit 0
fi

# Reuse a preserved volume from a prior instance instead of silently creating
# another 4 TiB database. In-use volumes that are not on the tagged instance
# above are treated as an operator recovery case.
existing_data_volumes="$(awsr ec2 describe-volumes \
  --filters "Name=tag:Name,Values=${DATA_VOLUME_NAME}" Name=tag:Project,Values=LiquidationBot \
    Name=tag:Role,Values=NitroFullNode Name=status,Values=available,in-use \
  --output json)"
volume_count="$(jq '.Volumes | length' <<<"$existing_data_volumes")"
(( volume_count <= 1 )) || fail "Found ${volume_count} preserved tagged data volumes; refusing to choose or create another."
REUSE_DATA_VOLUME=false
DATA_VOLUME_ID=""
DATA_VOLUME_AZ=""
if (( volume_count == 1 )); then
  REUSE_DATA_VOLUME=true
  DATA_VOLUME_ID="$(jq -r '.Volumes[0].VolumeId' <<<"$existing_data_volumes")"
  DATA_VOLUME_AZ="$(jq -r '.Volumes[0].AvailabilityZone' <<<"$existing_data_volumes")"
  volume_state="$(jq -r '.Volumes[0].State' <<<"$existing_data_volumes")"
  volume_type="$(jq -r '.Volumes[0].VolumeType' <<<"$existing_data_volumes")"
  volume_size="$(jq -r '.Volumes[0].Size' <<<"$existing_data_volumes")"
  volume_iops="$(jq -r '.Volumes[0].Iops // 0' <<<"$existing_data_volumes")"
  volume_throughput="$(jq -r '.Volumes[0].Throughput // 0' <<<"$existing_data_volumes")"
  volume_encrypted="$(jq -r '.Volumes[0].Encrypted' <<<"$existing_data_volumes")"
  [[ "$volume_state" == available ]] || fail "Persistent volume ${DATA_VOLUME_ID} is ${volume_state}; inspect its attachment before retrying."
  [[ "$volume_type" == gp3 && "$volume_size" -ge 4096 && "$volume_iops" -ge 12000 && "$volume_throughput" -ge 750 && "$volume_encrypted" == true ]] \
    || fail "Preserved volume ${DATA_VOLUME_ID} is encrypted=${volume_encrypted}, ${volume_size} GiB ${volume_type}, ${volume_iops} IOPS, ${volume_throughput} MiB/s; review/upgrade it before reuse."
  echo "Found preserved data volume ${DATA_VOLUME_ID} in ${DATA_VOLUME_AZ}; it will be reused."
fi

if [[ -z "$ADMIN_CIDR" ]]; then
  admin_ip="$(curl --fail --silent --show-error --max-time 4 https://checkip.amazonaws.com 2>/dev/null | tr -d '[:space:]' || true)"
  if [[ "$admin_ip" =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]] && \
     python3 -c 'import ipaddress,sys; ipaddress.IPv4Address(sys.argv[1])' "$admin_ip" 2>/dev/null; then
    ADMIN_CIDR="${admin_ip}/32"
  fi
fi
if [[ -n "$ADMIN_CIDR" ]]; then
  python3 -c 'import ipaddress,sys; n=ipaddress.ip_network(sys.argv[1], strict=False); assert n.version == 4' "$ADMIN_CIDR" \
    || fail "ADMIN_CIDR must be a valid IPv4 CIDR."
fi

# Discover/reuse a dedicated local key. If a key already exists in AWS but its
# private half is unavailable locally, Session Manager remains the safe path.
if [[ -z "${ARBITRUM_KEY_NAME:-}" && -f "${HOME}/.ssh/mev-ohio" ]]; then
  if awsr ec2 describe-key-pairs --key-names mev-ohio >/dev/null 2>&1; then
    KEY_NAME=mev-ohio
    KEY_PATH="${HOME}/.ssh/mev-ohio"
  fi
fi
if [[ "$KEY_PATH" == "/tmp/arbitrum-one-node-key.pem" && -f "${HOME}/.ssh/${KEY_NAME}.pem" ]]; then
  KEY_PATH="${HOME}/.ssh/${KEY_NAME}.pem"
fi
keypair="$(awsr ec2 describe-key-pairs --key-names "$KEY_NAME" --output json 2>/dev/null || true)"
if [[ -z "$(jq -r '.KeyPairs[0].KeyPairId // empty' <<<"${keypair:-{}}" 2>/dev/null || true)" ]]; then
  install -d -m 0700 "$(dirname "$KEY_PATH")"
  umask 077
  awsr ec2 create-key-pair --key-name "$KEY_NAME" --key-type ed25519 \
    --tag-specifications "ResourceType=key-pair,Tags=[{Key=Name,Value=arbitrum-one-node-key},{Key=Project,Value=LiquidationBot},{Key=Role,Value=NitroFullNode},{Key=Chain,Value=ArbitrumOne},{Key=ManagedBy,Value=aws-cli}]" \
    --query KeyMaterial --output text >"$KEY_PATH"
  chmod 0600 "$KEY_PATH"
  echo "Created dedicated EC2 key pair ${KEY_NAME}; private key stored mode 0600 at ${KEY_PATH}."
elif [[ -f "$KEY_PATH" ]]; then
  chmod 0600 "$KEY_PATH"
  echo "Reusing EC2 key pair ${KEY_NAME} with local private key ${KEY_PATH}."
else
  KEY_NAME="${KEY_NAME}-${REGION}"
  KEY_PATH="/tmp/${KEY_NAME}.pem"
  keypair="$(awsr ec2 describe-key-pairs --key-names "$KEY_NAME" --output json 2>/dev/null || true)"
  if [[ -z "$(jq -r '.KeyPairs[0].KeyPairId // empty' <<<"${keypair:-{}}" 2>/dev/null || true)" ]]; then
    install -d -m 0700 "$(dirname "$KEY_PATH")"
    umask 077
    awsr ec2 create-key-pair --key-name "$KEY_NAME" --key-type ed25519 \
      --tag-specifications "ResourceType=key-pair,Tags=[{Key=Name,Value=arbitrum-one-node-key},{Key=Project,Value=LiquidationBot},{Key=Role,Value=NitroFullNode},{Key=Chain,Value=ArbitrumOne},{Key=ManagedBy,Value=aws-cli}]" \
      --query KeyMaterial --output text >"$KEY_PATH"
    chmod 0600 "$KEY_PATH"
    echo "Existing key pair had no local private half; created ${KEY_NAME}, mode 0600 at ${KEY_PATH}."
  elif [[ -f "$KEY_PATH" ]]; then
    chmod 0600 "$KEY_PATH"
  else
    echo "EC2 key pair ${KEY_NAME} exists but its private key is unavailable; SSH will remain closed and Session Manager will be used."
  fi
fi

# Select only x86_64 EBS-optimized memory instances meeting 8 vCPU / 64 GiB.
TYPE_META='{}'
SELECTED_TYPE=""
CANDIDATES=(r7i.2xlarge r8i.2xlarge r7a.2xlarge r7iz.2xlarge r6i.2xlarge)
for candidate in "${CANDIDATES[@]}"; do
  info="$(awsr ec2 describe-instance-types --instance-types "$candidate" --output json 2>/dev/null || true)"
  if ! jq -e '.InstanceTypes[0] != null and
      (.InstanceTypes[0].ProcessorInfo.SupportedArchitectures | index("x86_64") != null) and
      .InstanceTypes[0].VCpuInfo.DefaultVCpus >= 8 and
      .InstanceTypes[0].MemoryInfo.SizeInMiB >= 65536 and
      .InstanceTypes[0].EbsInfo.EbsOptimizedSupport != "unsupported"' \
      >/dev/null <<<"${info:-{}}"; then
    continue
  fi
  offered="$(awsr ec2 describe-instance-type-offerings \
    --location-type availability-zone \
    --filters "Name=instance-type,Values=${candidate}" \
    --query 'InstanceTypeOfferings[].Location' --output json)"
  if [[ "$REUSE_DATA_VOLUME" == true ]]; then
    offered="$(jq --arg az "$DATA_VOLUME_AZ" '[.[] | select(. == $az)]' <<<"$offered")"
  fi
  if [[ "$(jq 'length' <<<"$offered")" -gt 0 ]]; then
    SELECTED_TYPE="$candidate"
    TYPE_META="$(jq --arg type "$candidate" --argjson info "$(jq '.InstanceTypes[0]' <<<"$info")" \
      --argjson azs "$offered" '. + {($type): {info:$info, azs:$azs}}' <<<"$TYPE_META")"
    break
  fi
done
[[ -n "$SELECTED_TYPE" ]] || fail "No offered x86_64 R-family type met 8 vCPU / 64 GiB and EBS-optimization requirements."

AMI_PARAM="/aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id"
AMI_ID="$(awsr ssm get-parameter --name "$AMI_PARAM" --query 'Parameter.Value' --output text)"
AMI="$(awsr ec2 describe-images --image-ids "$AMI_ID" --output json)"
AMI_ARCH="$(jq -r '.Images[0].Architecture // ""' <<<"$AMI")"
AMI_OWNER="$(jq -r '.Images[0].OwnerId // ""' <<<"$AMI")"
AMI_STATE="$(jq -r '.Images[0].State // ""' <<<"$AMI")"
ROOT_DEVICE="$(jq -r '.Images[0].RootDeviceName // ""' <<<"$AMI")"
[[ "$AMI_ARCH" == "x86_64" && "$AMI_OWNER" == "099720109477" && "$AMI_STATE" == "available" ]] \
  || fail "Ubuntu SSM AMI architecture/owner/state validation failed (${AMI_ARCH}/${AMI_OWNER}/${AMI_STATE})."

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
    if [[ "$REUSE_DATA_VOLUME" == true && "$az" != "$DATA_VOLUME_AZ" ]]; then
      continue
    fi
    route_id="$(jq -r --arg subnet "$subnet" \
      '.RouteTables[] | select(any(.Associations[]?; .SubnetId == $subnet)) | .RouteTableId' <<<"$routes" | head -n1)"
    if [[ -z "$route_id" ]]; then
      route_id="$(jq -r '.RouteTables[] | select(any(.Associations[]?; .Main == true)) | .RouteTableId' <<<"$routes" | head -n1)"
    fi
    [[ -n "$route_id" ]] || continue
    route="$(jq -c --arg id "$route_id" '.RouteTables[] | select(.RouteTableId == $id) | .Routes' <<<"$routes")"
    public="$(jq -r 'any(.[]; .DestinationCidrBlock == "0.0.0.0/0" and ((.GatewayId // "") | startswith("igw-")) and .State == "active")' <<<"$route")"
    private="$(jq -r 'any(.[]; .DestinationCidrBlock == "0.0.0.0/0" and ((.NatGatewayId // "") | startswith("nat-")) and .State == "active")' <<<"$route")"
    if [[ "$public" == true ]] && ! jq -e --arg az "$az" --arg type "$SELECTED_TYPE" '.[$type].azs | index($az) != null' >/dev/null <<<"$TYPE_META"; then
      continue
    fi
    if [[ "$public" == true ]]; then
      NETWORK_MODE=public
    elif [[ "$private" == true ]] && jq -e --arg az "$az" --arg type "$SELECTED_TYPE" '.[$type].azs | index($az) != null' >/dev/null <<<"$TYPE_META"; then
      NETWORK_MODE=private
    else
      continue
    fi
    VPC_ID="$vpc"
    SUBNET_ID="$subnet"
    AZ="$az"
    return 0
  done < <(jq -r '.Subnets[] | [.SubnetId,.AvailabilityZone] | @tsv' <<<"$subnets")
  return 1
}

# Prefer public subnets in existing VPCs, then private subnets with NAT egress.
while read -r vpc_id; do
  [[ -n "$vpc_id" ]] || continue
  if try_vpc "$vpc_id" && [[ "$NETWORK_MODE" == public ]]; then break; fi
done < <(jq -r '.Vpcs | sort_by(if .IsDefault then 0 else 1 end) | .[].VpcId' <<<"$vpcs")
if [[ -z "$SUBNET_ID" ]]; then
  while read -r vpc_id; do
    [[ -n "$vpc_id" ]] || continue
    if try_vpc "$vpc_id"; then break; fi
  done < <(jq -r '.Vpcs[].VpcId' <<<"$vpcs")
fi

if [[ -z "$SUBNET_ID" ]]; then
  # Resume a partially created dedicated VPC or make a new minimal public VPC.
  VPC_ID="$(jq -r '.Vpcs[] | select(any(.Tags[]?; .Key == "Name" and .Value == "arbitrum-one-vpc")) | .VpcId' <<<"$vpcs" | head -n1)"
  if [[ -z "$VPC_ID" ]]; then
    VPC_ID="$(awsr ec2 create-vpc --cidr-block 10.72.0.0/16 \
      --tag-specifications 'ResourceType=vpc,Tags=[{Key=Name,Value=arbitrum-one-vpc},{Key=Project,Value=LiquidationBot},{Key=Role,Value=NitroFullNode},{Key=Chain,Value=ArbitrumOne},{Key=ManagedBy,Value=aws-cli}]' \
      --query Vpc.VpcId --output text)"
  fi
  awsr ec2 modify-vpc-attribute --vpc-id "$VPC_ID" --enable-dns-support '{"Value":true}'
  awsr ec2 modify-vpc-attribute --vpc-id "$VPC_ID" --enable-dns-hostnames '{"Value":true}'
  awsr ec2 wait vpc-available --vpc-ids "$VPC_ID"

  subnet_tagged="$(awsr ec2 describe-subnets --filters "Name=vpc-id,Values=${VPC_ID}" \
    "Name=tag:Name,Values=arbitrum-one-public-subnet" --query 'Subnets[0].SubnetId' --output text)"
  if [[ "$REUSE_DATA_VOLUME" == true && -n "$subnet_tagged" && "$subnet_tagged" != "None" ]]; then
    subnet_az="$(awsr ec2 describe-subnets --subnet-ids "$subnet_tagged" --query 'Subnets[0].AvailabilityZone' --output text)"
    [[ "$subnet_az" == "$DATA_VOLUME_AZ" ]] || subnet_tagged=None
  fi
  if [[ -n "$subnet_tagged" && "$subnet_tagged" != "None" ]]; then
    SUBNET_ID="$subnet_tagged"
    AZ="$(awsr ec2 describe-subnets --subnet-ids "$SUBNET_ID" --query 'Subnets[0].AvailabilityZone' --output text)"
  else
    AZ="$DATA_VOLUME_AZ"
    if [[ "$REUSE_DATA_VOLUME" != true ]]; then
      AZ="$(jq -r --arg type "$SELECTED_TYPE" '.[$type].azs[0] // empty' <<<"$TYPE_META")"
    fi
    [[ -n "$AZ" ]] || fail "No AZ offers ${SELECTED_TYPE} for the dedicated VPC."
    subnet_cidr="10.72.1.0/24"
    if [[ "$REUSE_DATA_VOLUME" == true ]]; then
      existing_cidrs="$(awsr ec2 describe-subnets --filters "Name=vpc-id,Values=${VPC_ID}" --query 'Subnets[].CidrBlock' --output text)"
      for cidr_octet in $(seq 1 254); do
        candidate_cidr="10.72.${cidr_octet}.0/24"
        if [[ " ${existing_cidrs//[$'\t\n']/ } " != *" ${candidate_cidr} "* ]]; then
          subnet_cidr="$candidate_cidr"
          break
        fi
      done
      [[ " ${existing_cidrs//[$'\t\n']/ } " != *" ${subnet_cidr} "* ]] || fail "No free /24 CIDR remains in the dedicated VPC."
    fi
    SUBNET_ID="$(awsr ec2 create-subnet --vpc-id "$VPC_ID" --cidr-block "$subnet_cidr" --availability-zone "$AZ" \
      --tag-specifications 'ResourceType=subnet,Tags=[{Key=Name,Value=arbitrum-one-public-subnet},{Key=Project,Value=LiquidationBot},{Key=Role,Value=NitroFullNode},{Key=Chain,Value=ArbitrumOne},{Key=ManagedBy,Value=aws-cli}]' \
      --query Subnet.SubnetId --output text)"
  fi
  awsr ec2 modify-subnet-attribute --subnet-id "$SUBNET_ID" --map-public-ip-on-launch

  igw_id="$(awsr ec2 describe-internet-gateways --filters "Name=attachment.vpc-id,Values=${VPC_ID}" --query 'InternetGateways[0].InternetGatewayId' --output text)"
  if [[ -z "$igw_id" || "$igw_id" == "None" ]]; then
    igw_id="$(awsr ec2 create-internet-gateway \
      --tag-specifications 'ResourceType=internet-gateway,Tags=[{Key=Name,Value=arbitrum-one-igw},{Key=Project,Value=LiquidationBot},{Key=Role,Value=NitroFullNode},{Key=Chain,Value=ArbitrumOne},{Key=ManagedBy,Value=aws-cli}]' \
      --query InternetGateway.InternetGatewayId --output text)"
    awsr ec2 attach-internet-gateway --internet-gateway-id "$igw_id" --vpc-id "$VPC_ID"
  fi
  rt_id="$(awsr ec2 describe-route-tables --filters "Name=vpc-id,Values=${VPC_ID}" \
    "Name=tag:Name,Values=arbitrum-one-public-rt" --query 'RouteTables[0].RouteTableId' --output text)"
  if [[ -z "$rt_id" || "$rt_id" == "None" ]]; then
    rt_id="$(awsr ec2 create-route-table --vpc-id "$VPC_ID" \
      --tag-specifications 'ResourceType=route-table,Tags=[{Key=Name,Value=arbitrum-one-public-rt},{Key=Project,Value=LiquidationBot},{Key=Role,Value=NitroFullNode},{Key=Chain,Value=ArbitrumOne},{Key=ManagedBy,Value=aws-cli}]' \
      --query RouteTable.RouteTableId --output text)"
  fi
  awsr ec2 create-route --route-table-id "$rt_id" --destination-cidr-block 0.0.0.0/0 --gateway-id "$igw_id" >/dev/null || true
  association="$(awsr ec2 describe-route-tables --route-table-ids "$rt_id" --query "RouteTables[0].Associations[?SubnetId=='${SUBNET_ID}'].RouteTableAssociationId | [0]" --output text)"
  if [[ -z "$association" || "$association" == "None" ]]; then
    awsr ec2 associate-route-table --route-table-id "$rt_id" --subnet-id "$SUBNET_ID" >/dev/null
  fi
  NETWORK_MODE=public
fi

[[ -n "$VPC_ID" && -n "$SUBNET_ID" && -n "$AZ" ]] || fail "Could not choose a subnet with outbound Internet and instance-type availability."

existing_sg="$(awsr ec2 describe-security-groups --filters "Name=vpc-id,Values=${VPC_ID}" \
  "Name=group-name,Values=${SG_NAME}" --output json)"
SG_ID="$(jq -r '.SecurityGroups[0].GroupId // empty' <<<"$existing_sg")"
if [[ -n "$SG_ID" ]] && ! jq -e '(.SecurityGroups[0].Tags // []) as $t | any($t[]; .Key == "ManagedBy" and .Value == "aws-cli") and any($t[]; .Key == "Project" and .Value == "LiquidationBot")' >/dev/null <<<"$existing_sg"; then
  fail "Security group ${SG_NAME} already exists but is not tagged as managed by this deployment."
fi
if [[ -z "$SG_ID" || "$SG_ID" == "None" ]]; then
  SG_ID="$(awsr ec2 create-security-group --group-name "$SG_NAME" \
    --description 'Arbitrum One Nitro node; localhost RPC, restricted administration' --vpc-id "$VPC_ID" \
    --tag-specifications 'ResourceType=security-group,Tags=[{Key=Name,Value=arbitrum-one-node-sg},{Key=Project,Value=LiquidationBot},{Key=Role,Value=NitroFullNode},{Key=Chain,Value=ArbitrumOne},{Key=ManagedBy,Value=aws-cli}]' \
    --query GroupId --output text)"
fi
# The dedicated group is owned by this deployment: clear its ingress, then add
# at most one /32 SSH rule. Nitro RPC, WebSocket, feed, and Docker ports stay shut.
SG_RULES="$(awsr ec2 describe-security-groups --group-ids "$SG_ID" --output json | jq -c '.SecurityGroups[0].IpPermissions')"
if [[ "$SG_RULES" != "[]" && "$SG_RULES" != "null" ]]; then
  rule_file="$(mktemp)"
  printf '%s\n' "$SG_RULES" >"$rule_file"
  awsr ec2 revoke-security-group-ingress --group-id "$SG_ID" --ip-permissions "file://${rule_file}"
  rm -f "$rule_file"
fi
egress_rules="$(awsr ec2 describe-security-groups --group-ids "$SG_ID" --output json | jq -c '.SecurityGroups[0].IpPermissionsEgress')"
if ! jq -e 'any(.[]; .IpProtocol == "-1" and any(.IpRanges[]?; .CidrIp == "0.0.0.0/0"))' >/dev/null <<<"$egress_rules"; then
  awsr ec2 authorize-security-group-egress --group-id "$SG_ID" \
    --ip-permissions '[{"IpProtocol":"-1","IpRanges":[{"CidrIp":"0.0.0.0/0"}]}]' >/dev/null
fi
if [[ "$NETWORK_MODE" == public && -n "$ADMIN_CIDR" && -f "$KEY_PATH" ]]; then
  awsr ec2 authorize-security-group-ingress --group-id "$SG_ID" --protocol tcp --port 22 --cidr "$ADMIN_CIDR"
else
  echo "SSH ingress is disabled; use Systems Manager Session Manager."
fi

# Ensure the attached instance role has Session Manager plus narrowly scoped
# access to the two documented Nitro parent-chain SecureString parameters.
trust_file="$(mktemp)"
policy_file="$(mktemp)"
jq -n '{Version:"2012-10-17",Statement:[{Effect:"Allow",Principal:{Service:"ec2.amazonaws.com"},Action:"sts:AssumeRole"}]}' >"$trust_file"
role_json="$(awsiam iam get-role --role-name "$ROLE_NAME" --output json 2>/dev/null || true)"
if [[ -z "$role_json" ]]; then
  awsiam iam create-role --role-name "$ROLE_NAME" \
    --assume-role-policy-document "file://${trust_file}" \
    --description 'Arbitrum node SSM access and scoped parent RPC SecureString reads' \
    --tags Key=Name,Value=arbitrum-one-node-ssm-role Key=Project,Value=LiquidationBot Key=Role,Value=NitroFullNode Key=Chain,Value=ArbitrumOne Key=ManagedBy,Value=aws-cli >/dev/null
else
  if ! jq -e '(.Role.Tags // []) as $t | any($t[]; .Key == "ManagedBy" and .Value == "aws-cli") and any($t[]; .Key == "Project" and .Value == "LiquidationBot")' >/dev/null <<<"$role_json"; then
    fail "IAM role ${ROLE_NAME} exists but is not tagged as managed by this deployment."
  fi
fi
awsiam iam attach-role-policy --role-name "$ROLE_NAME" --policy-arn arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore
jq -n --arg region "$REGION" --arg account "$ACCOUNT_ID" '{Version:"2012-10-17",Statement:[{Sid:"ReadOnlyNitroParentRpcParameters",Effect:"Allow",Action:["ssm:GetParameter"],Resource:("arn:aws:ssm:"+$region+":"+$account+":parameter/mev-bot/arbitrum/*")},{Sid:"DecryptOnlyThroughParameterStore",Effect:"Allow",Action:["kms:Decrypt"],Resource:"*",Condition:{StringEquals:{"kms:ViaService":("ssm."+$region+".amazonaws.com")},StringLike:{"kms:EncryptionContext:PARAMETER_ARN":("arn:aws:ssm:"+$region+":"+$account+":parameter/mev-bot/arbitrum/*")}}}]}' >"$policy_file"
awsiam iam put-role-policy --role-name "$ROLE_NAME" --policy-name ArbitrumParentRpcParameterRead --policy-document "file://${policy_file}"
profile_json="$(awsiam iam get-instance-profile --instance-profile-name "$PROFILE_NAME" --output json 2>/dev/null || true)"
if [[ -z "$profile_json" ]]; then
  awsiam iam create-instance-profile --instance-profile-name "$PROFILE_NAME" \
    --tags Key=Name,Value=arbitrum-one-node-ssm-profile Key=Project,Value=LiquidationBot Key=Role,Value=NitroFullNode Key=Chain,Value=ArbitrumOne Key=ManagedBy,Value=aws-cli >/dev/null
else
  if ! jq -e '(.InstanceProfile.Tags // []) as $t | any($t[]; .Key == "ManagedBy" and .Value == "aws-cli") and any($t[]; .Key == "Project" and .Value == "LiquidationBot")' >/dev/null <<<"$profile_json"; then
    fail "IAM instance profile ${PROFILE_NAME} exists but is not tagged as managed by this deployment."
  fi
fi
profile_roles="$(awsiam iam get-instance-profile --instance-profile-name "$PROFILE_NAME" --query 'InstanceProfile.Roles[].RoleName' --output text)"
if [[ -n "$profile_roles" && "$profile_roles" != "None" && "$profile_roles" != "$ROLE_NAME" ]]; then
  fail "IAM instance profile ${PROFILE_NAME} already contains a different role (${profile_roles})."
fi
if [[ " $profile_roles " != *" ${ROLE_NAME} "* ]]; then
  awsiam iam add-role-to-instance-profile --instance-profile-name "$PROFILE_NAME" --role-name "$ROLE_NAME"
fi
rm -f "$trust_file" "$policy_file"

# Print cost and exact launch shape before the first billable EC2/EBS action.
INSTANCE_HOURLY=0.5292
DATA_STORAGE_MONTHLY=327.68
DATA_IOPS_MONTHLY=65.00
DATA_THROUGHPUT_MONTHLY=35.00
ROOT_MONTHLY=4.00
PUBLIC_IP_MONTHLY=0.00
if [[ "$NETWORK_MODE" == public ]]; then PUBLIC_IP_MONTHLY=3.65; fi
ESTIMATE="$(awk -v i="$INSTANCE_HOURLY" -v d="$DATA_STORAGE_MONTHLY" -v p="$DATA_IOPS_MONTHLY" -v t="$DATA_THROUGHPUT_MONTHLY" -v r="$ROOT_MONTHLY" -v ip="$PUBLIC_IP_MONTHLY" 'BEGIN { printf "%.2f", i*730+d+p+t+r+ip }')"
echo
echo "PRE-LAUNCH RESOURCE / COST PLAN"
echo "  EC2: ${SELECTED_TYPE} (On-Demand Linux, ${REGION}/${AZ}; x86_64; 8+ vCPU / 64+ GiB)"
echo "  Ubuntu: 24.04 LTS Canonical SSM public AMI ${AMI_ID} (${AMI_ARCH})"
echo "  Root: ${ROOT_SIZE} GiB encrypted gp3, delete on termination"
if [[ "$REUSE_DATA_VOLUME" == true ]]; then
  echo "  Data: reuse ${DATA_VOLUME_ID}, ${volume_size} GiB encrypted gp3, ${volume_iops} IOPS, ${volume_throughput} MiB/s, preserve on termination"
else
  echo "  Data: new ${DATA_SIZE} GiB encrypted gp3, ${DATA_IOPS} IOPS, ${DATA_THROUGHPUT} MiB/s, preserve on termination"
fi
echo "  Network: ${VPC_ID}/${SUBNET_ID} (${NETWORK_MODE} egress); SSH ${ADMIN_CIDR:-closed}; RPC/WS/feed inbound closed"
echo "  Nitro: ${NITRO_IMAGE} (linux/amd64)"
echo "  Rough 730-hour monthly subtotal: \$${ESTIMATE} (EC2 + root/data gp3 performance + public IPv4 if assigned)"
echo "  Excludes Internet transfer, snapshots/backups, taxes, and unrelated account usage."
echo

temp_dir="$(mktemp -d)"
trap 'rm -rf -- "$temp_dir"' EXIT

store_parameter_if_needed() {
  local parameter_name="$1" variable_name="$2" existing overwrite
  existing="$(awsr ssm get-parameter --name "$parameter_name" --query 'Parameter.Name' --output text 2>/dev/null || true)"
  overwrite=false
  if [[ "$existing" != "None" && -n "$existing" ]]; then
    if [[ "${ARBITRUM_OVERWRITE_SSM_PARAMETERS:-0}" == "1" ]]; then
      overwrite=true
    else
      echo "Preserving existing SSM SecureString ${parameter_name}; local environment value was not copied."
      return 0
    fi
  fi
  local input_file="${temp_dir}/ssm-parameter.json"
  python3 - "$parameter_name" "$variable_name" "$input_file" "$overwrite" <<'PY'
import json, os, sys
name, variable, filename, overwrite = sys.argv[1:]
value = os.environ.get(variable, "")
if not value:
    raise SystemExit(f"{variable} is empty")
with open(filename, "w", encoding="utf-8") as stream:
    json.dump({"Name": name, "Type": "SecureString", "Value": value,
               "Overwrite": overwrite == "true", "Tier": "Standard"}, stream)
PY
  chmod 0600 "$input_file"
  awsr ssm put-parameter --cli-input-json "file://${input_file}" >/dev/null
  echo "Stored ${variable_name} as an SSM SecureString at ${parameter_name}."
}

if [[ -n "${ETHEREUM_RPC_URL:-}" && -n "${ETHEREUM_BEACON_URL:-}" ]]; then
  store_parameter_if_needed /mev-bot/arbitrum/ethereum-rpc-url ETHEREUM_RPC_URL
  store_parameter_if_needed /mev-bot/arbitrum/ethereum-beacon-url ETHEREUM_BEACON_URL
elif [[ -n "${ETHEREUM_RPC_URL:-}" || -n "${ETHEREUM_BEACON_URL:-}" ]]; then
  echo "Only one Ethereum endpoint is set locally; neither value was copied to SSM, so Nitro will remain stopped."
fi

jq -n --arg root "$ROOT_DEVICE" --argjson root_size "$ROOT_SIZE" \
  --argjson data_size "$DATA_SIZE" --argjson iops "$DATA_IOPS" --argjson throughput "$DATA_THROUGHPUT" \
  --argjson reuse "$REUSE_DATA_VOLUME" \
  '[{DeviceName:$root,Ebs:{VolumeSize:$root_size,VolumeType:"gp3",Encrypted:true,DeleteOnTermination:true}}] + (if $reuse then [] else [{DeviceName:"/dev/sdf",Ebs:{VolumeSize:$data_size,VolumeType:"gp3",Iops:$iops,Throughput:$throughput,Encrypted:true,DeleteOnTermination:false}}] end)' \
  >"${temp_dir}/block-device-mappings.json"
jq -n --arg data_name "$DATA_VOLUME_NAME" '[{ResourceType:"instance",Tags:[{Key:"Name",Value:"arbitrum-one-node"},{Key:"Project",Value:"LiquidationBot"},{Key:"Role",Value:"NitroFullNode"},{Key:"Chain",Value:"ArbitrumOne"},{Key:"ManagedBy",Value:"aws-cli"}]},{ResourceType:"volume",Tags:[{Key:"Name",Value:$data_name},{Key:"Project",Value:"LiquidationBot"},{Key:"Role",Value:"NitroFullNode"},{Key:"Chain",Value:"ArbitrumOne"},{Key:"ManagedBy",Value:"aws-cli"}]}]' \
  >"${temp_dir}/tag-specifications.json"

# The user-data API has a 16 KiB limit. Gzip a local bundle of the audited
# bootstrap/config/service files, then cloud-init unpacks and runs it.
tar -czf "${temp_dir}/arbitrum-bootstrap.tar.gz" -C "$SCRIPT_DIR" \
  bootstrap-arbitrum-node.sh nitro-config.json nitro.env.example nitro-node.service nitro-start.sh arbitrum-status
bundle="$(base64 -w0 "${temp_dir}/arbitrum-bootstrap.tar.gz")"
cat >"${temp_dir}/user-data.sh" <<EOF
#!/bin/bash
set -Eeuo pipefail
install -d -m 0755 /opt/arbitrum-node-bootstrap
printf '%s' '${bundle}' | base64 -d | tar -xz -C /opt/arbitrum-node-bootstrap --no-same-owner
exec /bin/bash /opt/arbitrum-node-bootstrap/bootstrap-arbitrum-node.sh
EOF
chmod 0600 "${temp_dir}/user-data.sh"
userdata_size="$(wc -c <"${temp_dir}/user-data.sh")"
(( userdata_size <= 16384 )) || fail "Compressed EC2 user-data is ${userdata_size} bytes; AWS limit is 16 KiB."

network_args=(--subnet-id "$SUBNET_ID" --security-group-ids "$SG_ID")
if [[ "$NETWORK_MODE" == public ]]; then
  network_args+=(--associate-public-ip-address)
fi
run_args=(
  --image-id "$AMI_ID"
  --instance-type "$SELECTED_TYPE"
  --count 1
  --block-device-mappings "file://${temp_dir}/block-device-mappings.json"
  --tag-specifications "file://${temp_dir}/tag-specifications.json"
  --iam-instance-profile "Name=${PROFILE_NAME}"
  --metadata-options "HttpTokens=required,HttpEndpoint=enabled,HttpPutResponseHopLimit=2"
  --user-data "file://${temp_dir}/user-data.sh"
  --instance-initiated-shutdown-behavior stop
  --key-name "$KEY_NAME"
)
awsr ec2 run-instances "${run_args[@]}" "${network_args[@]}" >"${temp_dir}/run-instances.json"
INSTANCE_ID="$(jq -r '.Instances[0].InstanceId' "${temp_dir}/run-instances.json")"
[[ -n "$INSTANCE_ID" && "$INSTANCE_ID" != "null" ]] || fail "run-instances returned no instance ID."

if [[ "$REUSE_DATA_VOLUME" == true ]]; then
  echo "Attaching preserved data volume ${DATA_VOLUME_ID} to ${INSTANCE_ID} before bootstrap proceeds."
  awsr ec2 attach-volume --volume-id "$DATA_VOLUME_ID" --instance-id "$INSTANCE_ID" --device /dev/sdf >/dev/null
fi

# Persist IDs immediately, before waits, so a timeout never causes a duplicate.
cat >"$STATE_FILE" <<EOF
INSTANCE_ID=${INSTANCE_ID}
INSTANCE_TYPE=${SELECTED_TYPE}
REGION=${REGION}
AZ=${AZ}
PUBLIC_IP=
PRIVATE_IP=
DATA_VOLUME_ID=${DATA_VOLUME_ID}
SECURITY_GROUP_ID=${SG_ID}
EOF
chmod 0600 "$STATE_FILE"
echo "Launched ${INSTANCE_ID}; waiting for EC2 running and status checks."
awsr ec2 wait instance-running --instance-ids "$INSTANCE_ID"
awsr ec2 wait instance-status-ok --instance-ids "$INSTANCE_ID"

instance="$(awsr ec2 describe-instances --instance-ids "$INSTANCE_ID" --output json)"
PUBLIC_IP="$(jq -r '.Reservations[0].Instances[0].PublicIpAddress // ""' <<<"$instance")"
PRIVATE_IP="$(jq -r '.Reservations[0].Instances[0].PrivateIpAddress // ""' <<<"$instance")"
ATTACHED_DATA_VOLUME_ID="$(jq -r '.Reservations[0].Instances[0].BlockDeviceMappings[]? | select(.DeviceName == "/dev/sdf") | .Ebs.VolumeId' <<<"$instance" | head -n1)"
if [[ -n "$ATTACHED_DATA_VOLUME_ID" && "$ATTACHED_DATA_VOLUME_ID" != "null" ]]; then
  DATA_VOLUME_ID="$ATTACHED_DATA_VOLUME_ID"
fi
ROOT_VOLUME_ID="$(jq -r --arg root "$ROOT_DEVICE" '.Reservations[0].Instances[0].BlockDeviceMappings[]? | select(.DeviceName == $root) | .Ebs.VolumeId' <<<"$instance" | head -n1)"
[[ -n "$DATA_VOLUME_ID" && "$DATA_VOLUME_ID" != "null" ]] || fail "Data volume ID is missing after launch; instance ${INSTANCE_ID} is retained for recovery."
awsr ec2 create-tags --resources "$INSTANCE_ID" "$ROOT_VOLUME_ID" \
  --tags Key=Name,Value=arbitrum-one-node Key=Project,Value=LiquidationBot Key=Role,Value=NitroFullNode Key=Chain,Value=ArbitrumOne Key=ManagedBy,Value=aws-cli
awsr ec2 create-tags --resources "$DATA_VOLUME_ID" \
  --tags Key=Name,Value=arbitrum-one-node-data Key=Project,Value=LiquidationBot Key=Role,Value=NitroFullNode Key=Chain,Value=ArbitrumOne Key=ManagedBy,Value=aws-cli
cat >"$STATE_FILE" <<EOF
INSTANCE_ID=${INSTANCE_ID}
INSTANCE_TYPE=${SELECTED_TYPE}
REGION=${REGION}
AZ=${AZ}
PUBLIC_IP=${PUBLIC_IP}
PRIVATE_IP=${PRIVATE_IP}
DATA_VOLUME_ID=${DATA_VOLUME_ID}
SECURITY_GROUP_ID=${SG_ID}
VPC_ID=${VPC_ID}
SUBNET_ID=${SUBNET_ID}
ROOT_VOLUME_ID=${ROOT_VOLUME_ID}
NITRO_IMAGE=${NITRO_IMAGE}
EOF
chmod 0600 "$STATE_FILE"

echo "EC2 is healthy. Bootstrap continues under cloud-init; data volume is ${DATA_VOLUME_ID}."
echo "Waiting briefly for the SSM agent and bootstrap completion."
SSM_ID=""
for _ in $(seq 1 48); do
  SSM_ID="$(awsr ssm describe-instance-information \
    --filters "Key=InstanceIds,Values=${INSTANCE_ID}" \
    --query 'InstanceInformationList[0].InstanceId' --output text 2>/dev/null || true)"
  [[ "$SSM_ID" == "$INSTANCE_ID" ]] && break
  sleep 5
done

if [[ "$SSM_ID" == "$INSTANCE_ID" ]]; then
  echo "SSM is online. Bootstrap log tail:"
  command_id="$(awsr ssm send-command --instance-ids "$INSTANCE_ID" \
    --document-name AWS-RunShellScript \
    --parameters 'commands=["tail -n 35 /var/log/arbitrum-bootstrap.log 2>/dev/null || tail -n 35 /var/log/cloud-init-output.log"]' \
    --query 'Command.CommandId' --output text)"
  for _ in $(seq 1 24); do
    invocation="$(awsr ssm get-command-invocation --command-id "$command_id" --instance-id "$INSTANCE_ID" --output json 2>/dev/null || true)"
    status="$(jq -r '.Status // "Pending"' <<<"${invocation:-{}}" 2>/dev/null || echo Pending)"
    if [[ "$status" == Success || "$status" == Failed || "$status" == Cancelled || "$status" == TimedOut ]]; then
      jq -r '.StandardOutputContent // .StandardErrorContent // empty' <<<"$invocation"
      break
    fi
    sleep 5
  done
else
  echo "SSM agent is not online yet; cloud-init log remains available through SSH if restricted ingress was configured."
fi

echo
echo "INSTANCE_ID=${INSTANCE_ID}"
echo "INSTANCE_TYPE=${SELECTED_TYPE}"
echo "REGION=${REGION}"
echo "AZ=${AZ}"
echo "PUBLIC_IP=${PUBLIC_IP}"
echo "PRIVATE_IP=${PRIVATE_IP}"
echo "DATA_VOLUME_ID=${DATA_VOLUME_ID}"
echo "DATA_MOUNT=/data/arbitrum"
echo "SECURITY_GROUP_ID=${SG_ID}"
if [[ "$NETWORK_MODE" == public && -n "$ADMIN_CIDR" && -f "$KEY_PATH" ]]; then
  echo "SSH_COMMAND=ssh -i ${KEY_PATH} -o IdentitiesOnly=yes ubuntu@${PUBLIC_IP}"
else
  echo "SSH_COMMAND=aws --profile ${PROFILE} --region ${REGION} ssm start-session --target ${INSTANCE_ID}"
fi
echo "RPC_HTTP=http://127.0.0.1:8547"
echo "RPC_WS=ws://127.0.0.1:8548"
echo "RESOURCE_STATE=${STATE_FILE}"
echo "Nitro starts after both Ethereum parent RPC and beacon endpoints are supplied."
