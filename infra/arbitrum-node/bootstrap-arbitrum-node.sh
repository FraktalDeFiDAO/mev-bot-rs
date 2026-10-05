#!/usr/bin/env bash
set -Eeuo pipefail

if [[ "${EUID}" -ne 0 ]]; then
  echo "Run this bootstrap as root." >&2
  exit 1
fi

exec > >(tee -a /var/log/arbitrum-bootstrap.log) 2>&1
echo "Arbitrum node bootstrap started at $(date -Is)"

if [[ "$(dpkg --print-architecture)" != "amd64" ]]; then
  echo "Expected x86_64/amd64 Ubuntu; refusing incompatible bootstrap." >&2
  exit 1
fi

export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends \
  awscli curl jq nvme-cli xfsprogs sysstat iotop htop tmux \
  fio ripgrep smartmontools docker.io snapd

systemctl enable --now docker
if command -v snap >/dev/null 2>&1; then
  snap install amazon-ssm-agent --classic || true
  systemctl enable --now snap.amazon-ssm-agent.amazon-ssm-agent.service || true
fi

# Find the EBS disk mapped as /dev/sdf. Nitro presents it as an NVMe device.
data_disk=""
for _ in $(seq 1 60); do
  root_source="$(readlink -f "$(findmnt -n -o SOURCE /)" 2>/dev/null || findmnt -n -o SOURCE /)"
  root_parent="$(lsblk -no PKNAME "$root_source" 2>/dev/null | head -n1 || true)"
  if [[ -n "$root_parent" ]]; then
    root_disk="/dev/${root_parent}"
  else
    root_disk="$root_source"
  fi
  candidates=()
  while read -r disk; do
    [[ -n "$disk" && "/dev/${disk}" != "$root_disk" ]] || continue
    mapping=""
    if command -v ebsnvme-id >/dev/null 2>&1; then
      mapping="$(ebsnvme-id -v "/dev/${disk}" 2>/dev/null | sed -n 's/^Block device mapping: //p' | tail -n1 || true)"
    fi
    if [[ "$mapping" == "/dev/sdf" ]]; then
      candidates=("/dev/${disk}")
      break
    fi
    candidates+=("/dev/${disk}")
  done < <(lsblk -dn -o NAME,TYPE | awk '$2 == "disk" {print $1}')

  if [[ ${#candidates[@]} -eq 1 ]]; then
    data_disk="${candidates[0]}"
    break
  fi
  sleep 5
done

if [[ -z "$data_disk" ]]; then
  echo "Unable to identify exactly one non-root EBS data disk; refusing to format anything." >&2
  lsblk -f >&2 || true
  exit 1
fi

partitions=()
while read -r name; do
  [[ -n "$name" ]] && partitions+=("$name")
done < <(lsblk -nrpo NAME,TYPE "$data_disk" | awk '$2 == "part" {print $1}')

if [[ ${#partitions[@]} -gt 1 ]]; then
  echo "The attached EBS disk has multiple partitions; refusing ambiguous mount." >&2
  lsblk -f "$data_disk" >&2
  exit 1
fi
if [[ ${#partitions[@]} -eq 1 ]]; then
  data_device="${partitions[0]}"
else
  data_device="$data_disk"
fi

filesystem="$(blkid -s TYPE -o value "$data_device" 2>/dev/null || true)"
if [[ -z "$filesystem" ]]; then
  signatures="$(wipefs --no-act --output TYPE "$data_device" 2>/dev/null | tail -n +2 | awk 'NF' || true)"
  if [[ -n "$signatures" ]]; then
    echo "The EBS disk has existing signatures (${signatures}); preserving it and stopping for review." >&2
    exit 1
  fi
  echo "Formatting blank EBS data device ${data_device} as XFS."
  mkfs.xfs -L arbitrum-data "$data_device"
  filesystem=xfs
fi
if [[ "$filesystem" != "xfs" && "$filesystem" != "ext4" ]]; then
  echo "Unsupported existing data filesystem '${filesystem}'; refusing to reformat." >&2
  exit 1
fi

data_uuid="$(blkid -s UUID -o value "$data_device")"
install -d -o root -g root -m 0755 /data/arbitrum
if grep -qE '[[:space:]]/data/arbitrum[[:space:]]' /etc/fstab; then
  if ! grep -qE "^UUID=${data_uuid}[[:space:]]+/data/arbitrum[[:space:]]" /etc/fstab; then
    echo "/etc/fstab already has a different /data/arbitrum entry; refusing to replace it." >&2
    exit 1
  fi
else
  printf 'UUID=%s /data/arbitrum %s defaults,noatime,nofail,x-systemd.device-timeout=120s 0 2\n' \
    "$data_uuid" "$filesystem" >> /etc/fstab
fi
mount -a
if ! mountpoint -q /data/arbitrum; then
  echo "EBS data volume did not mount at /data/arbitrum." >&2
  exit 1
fi
mounted_uuid="$(findmnt -n -o UUID --target /data/arbitrum)"
if [[ "$mounted_uuid" != "$data_uuid" ]]; then
  echo "Mounted UUID ${mounted_uuid} does not match expected ${data_uuid}." >&2
  exit 1
fi

install -d -o 1000 -g 1000 -m 0750 /data/arbitrum /data/arbitrum/snapshot-download
install -d -o root -g root -m 0755 /etc/arbitrum
install -m 0644 "$(dirname "$0")/nitro-config.json" /etc/arbitrum/nitro-config.json
if [[ ! -e /etc/arbitrum/nitro.env ]]; then
  install -m 0600 "$(dirname "$0")/nitro.env.example" /etc/arbitrum/nitro.env
else
  chmod 0600 /etc/arbitrum/nitro.env
fi
install -m 0755 "$(dirname "$0")/nitro-start.sh" /usr/local/sbin/arbitrum-nitro-run
install -m 0644 "$(dirname "$0")/nitro-node.service" /etc/systemd/system/arbitrum-nitro.service
install -m 0755 "$(dirname "$0")/arbitrum-status" /usr/local/bin/arbitrum-status

install -d -m 0755 /etc/systemd/journald.conf.d /etc/sysctl.d
cat >/etc/systemd/journald.conf.d/arbitrum.conf <<'EOF'
[Journal]
SystemMaxUse=1G
RuntimeMaxUse=256M
MaxRetentionSec=14day
EOF
cat >/etc/sysctl.d/99-arbitrum.conf <<'EOF'
# Conservative database-host setting; Nitro and Docker retain uncapped access to RAM.
vm.swappiness=10
EOF
sysctl --system
systemctl restart systemd-journald
systemctl enable --now fstrim.timer

echo "Pulling the pinned x86_64 Nitro image."
docker pull --platform linux/amd64 offchainlabs/nitro-node:v3.12.1-70fa99a
image_arch="$(docker image inspect offchainlabs/nitro-node:v3.12.1-70fa99a --format '{{.Architecture}}')"
if [[ "$image_arch" != "amd64" ]]; then
  echo "Expected Nitro image architecture amd64, received ${image_arch}." >&2
  exit 1
fi

systemctl daemon-reload
systemctl enable docker.service arbitrum-nitro.service
systemctl start arbitrum-nitro.service

echo "Arbitrum bootstrap finished at $(date -Is)."
echo "Data filesystem: UUID=${data_uuid} ${filesystem} mounted at /data/arbitrum"
echo "Nitro image: offchainlabs/nitro-node:v3.12.1-70fa99a (${image_arch})"
echo "Nitro startup state: $(systemctl is-active arbitrum-nitro.service || true)"
echo "Parent-chain endpoints are required before Nitro can start and initialize its snapshot."
