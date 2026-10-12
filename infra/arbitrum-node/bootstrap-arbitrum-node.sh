#!/usr/bin/env bash
set -Eeuo pipefail
[[ "${EUID}" -eq 0 ]] || { echo 'Run this bootstrap as root.' >&2; exit 1; }
exec > >(tee -a /var/log/arbitrum-bootstrap.log) 2>&1
trap 'echo "Bootstrap failed at line ${LINENO}; inspect /var/log/arbitrum-bootstrap.log." >&2' ERR
echo "Arbitrum node bootstrap started at $(date -Is)"
[[ "$(dpkg --print-architecture)" == amd64 ]] || { echo 'Expected Ubuntu amd64; refusing incompatible host.' >&2; exit 1; }

export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends ca-certificates curl unzip less groff-base jq nvme-cli xfsprogs sysstat iotop htop tmux fio smartmontools docker.io snapd python3
# Ubuntu 24.04 does not publish an awscli apt package. Use AWS's official
# x86_64 CLI v2 installer; the node uses its role to read scoped SSM parameters.
export PATH="/usr/local/bin:${PATH}"
if ! aws --version 2>/dev/null | grep -q '^aws-cli/2\.'; then
  (
    aws_install_tmp="$(mktemp -d /tmp/arbitrum-awscli.XXXXXXXX)"
    trap 'rm -rf -- "$aws_install_tmp"' EXIT
    curl --fail --silent --show-error --location --retry 3 \
      https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip \
      --output "$aws_install_tmp/awscliv2.zip"
    unzip -q "$aws_install_tmp/awscliv2.zip" -d "$aws_install_tmp"
    if [[ -d /usr/local/aws-cli ]]; then
      "$aws_install_tmp/aws/install" --install-dir /usr/local/aws-cli --bin-dir /usr/local/bin --update
    else
      "$aws_install_tmp/aws/install" --install-dir /usr/local/aws-cli --bin-dir /usr/local/bin
    fi
  )
fi
aws --version | grep '^aws-cli/2\.'
systemctl enable --now docker
if command -v snap >/dev/null 2>&1; then
  snap install amazon-ssm-agent --classic || true
  systemctl enable --now snap.amazon-ssm-agent.amazon-ssm-agent.service || true
fi

# EC2's requested /dev/sdf mapping appears as an NVMe disk. Exclude the root
# disk by its actual parent device; format only a blank, uniquely identified disk.
data_disk=""
expected_data_volume="${ARBITRUM_DATA_VOLUME_ID:-}"
if mountpoint -q /data/arbitrum; then
  data_disk="$(findmnt -n -o SOURCE /data/arbitrum)"
fi
for _ in $(seq 1 60); do
  [[ -z "$data_disk" ]] || break
  root_source="$(readlink -f "$(findmnt -n -o SOURCE /)" 2>/dev/null || findmnt -n -o SOURCE /)"
  root_parent="$(lsblk -no PKNAME "$root_source" 2>/dev/null | head -n1 || true)"
  if [[ -n "$root_parent" ]]; then root_disk="/dev/${root_parent}"; else root_disk="$root_source"; fi
  candidates=()
  while read -r disk; do
    [[ -n "$disk" && "/dev/${disk}" != "$root_disk" ]] || continue
    if [[ -n "$expected_data_volume" ]]; then
      serial="$(lsblk -dn -o SERIAL "/dev/${disk}" | tr -d '[:space:]-')"
      [[ "$serial" == "${expected_data_volume//-/}" ]] || continue
      candidates=("/dev/${disk}"); break
    fi
    mapping=""
    if command -v ebsnvme-id >/dev/null 2>&1; then
      mapping="$(ebsnvme-id -v "/dev/${disk}" 2>/dev/null | sed -n 's/^Block device mapping: //p' | tail -n1 || true)"
    fi
    if [[ "$mapping" == /dev/sdf ]]; then candidates=("/dev/${disk}"); break; fi
    candidates+=("/dev/${disk}")
  done < <(lsblk -dn -o NAME,TYPE | awk '$2 == "disk" {print $1}')
  if [[ ${#candidates[@]} -eq 1 ]]; then data_disk="${candidates[0]}"; break; fi
  sleep 5
done
if [[ -z "$data_disk" ]]; then
  echo 'Unable to identify exactly one non-root EBS disk; refusing to format anything.' >&2
  lsblk -f >&2 || true
  exit 1
fi

partitions=()
while read -r name; do [[ -n "$name" ]] && partitions+=("$name"); done \
  < <(lsblk -nrpo NAME,TYPE "$data_disk" | awk '$2 == "part" {print $1}')
if [[ ${#partitions[@]} -gt 1 ]]; then
  echo 'Data disk has multiple partitions; refusing ambiguous mount.' >&2
  lsblk -f "$data_disk" >&2
  exit 1
fi
if [[ ${#partitions[@]} -eq 1 ]]; then data_device="${partitions[0]}"; else data_device="$data_disk"; fi
filesystem="$(blkid -s TYPE -o value "$data_device" 2>/dev/null || true)"
if [[ -z "$filesystem" ]]; then
  signatures="$(wipefs --no-act --output TYPE "$data_device" 2>/dev/null | tail -n +2 | awk 'NF' || true)"
  [[ -z "$signatures" ]] || { echo "Existing disk signatures found (${signatures}); preserving disk." >&2; exit 1; }
  echo "Formatting blank EBS device ${data_device} as XFS."
  mkfs.xfs -L arbitrum "$data_device"
  filesystem=xfs
fi
[[ "$filesystem" == xfs || "$filesystem" == ext4 ]] || { echo "Unsupported filesystem ${filesystem}; preserving disk." >&2; exit 1; }
data_uuid="$(blkid -s UUID -o value "$data_device")"
install -d -o root -g root -m 0755 /data/arbitrum
if grep -qE '[[:space:]]/data/arbitrum[[:space:]]' /etc/fstab; then
  grep -qE "^UUID=${data_uuid}[[:space:]]+/data/arbitrum[[:space:]]" /etc/fstab || {
    echo '/etc/fstab already has a conflicting /data/arbitrum entry.' >&2; exit 1;
  }
else
  printf 'UUID=%s /data/arbitrum %s defaults,noatime,nofail,x-systemd.device-timeout=120s 0 2\n' \
    "$data_uuid" "$filesystem" >> /etc/fstab
fi
mount -a
mountpoint -q /data/arbitrum || { echo 'EBS data volume failed to mount.' >&2; exit 1; }
mounted_uuid="$(findmnt -n -o UUID --target /data/arbitrum)"
[[ "$mounted_uuid" == "$data_uuid" ]] || { echo 'Mounted UUID does not match expected disk.' >&2; exit 1; }

install -d -o 1000 -g 1000 -m 0750 /data/arbitrum
# Nitro's default staging directory lives beneath persistent.chain on this
# mounted disk. Its built-in cleanup removes the joined archive after import.
install -d -o root -g root -m 0755 /etc/arbitrum
install -m 0644 "$(dirname "$0")/nitro-config.json" /etc/arbitrum/nitro-config.json
if [[ ! -e /etc/arbitrum/nitro.env ]]; then
  install -m 0600 "$(dirname "$0")/nitro.env.example" /etc/arbitrum/nitro.env
else chmod 0600 /etc/arbitrum/nitro.env; fi
install -m 0755 "$(dirname "$0")/nitro-start.sh" /usr/local/sbin/arbitrum-nitro-run
install -m 0644 "$(dirname "$0")/nitro-node.service" /etc/systemd/system/arbitrum-nitro.service
install -m 0755 "$(dirname "$0")/arbitrum-status" /usr/local/bin/arbitrum-status
install -m 0755 "$(dirname "$0")/arbitrum-snapshot-init.py" /usr/local/sbin/arbitrum-snapshot-init
install -m 0644 "$(dirname "$0")/arbitrum-snapshot-init.service" /etc/systemd/system/arbitrum-snapshot-init.service
install -m 0755 "$(dirname "$0")/arbitrum-parent-ready.py" /usr/local/sbin/arbitrum-parent-ready
install -m 0755 "$(dirname "$0")/arbitrum-activate.sh" /usr/local/sbin/arbitrum-activate
install -m 0644 "$(dirname "$0")/arbitrum-activate.service" "$(dirname "$0")/arbitrum-activate.timer" /etc/systemd/system/
install -m 0644 "$(dirname "$0")/arbitrum-health.service" "$(dirname "$0")/arbitrum-health.timer" /etc/systemd/system/

install -d -m 0755 /etc/systemd/journald.conf.d /etc/sysctl.d
cat >/etc/systemd/journald.conf.d/arbitrum.conf <<'EOF'
[Journal]
SystemMaxUse=1G
RuntimeMaxUse=256M
MaxRetentionSec=14day
EOF
cat >/etc/sysctl.d/99-arbitrum.conf <<'EOF'
# Conservative database-host setting; Nitro and Docker retain uncapped RAM.
vm.swappiness=10
EOF
sysctl --system
systemctl restart systemd-journald
systemctl enable --now fstrim.timer

IMAGE='offchainlabs/nitro-node:v3.12.1-70fa99a'
echo "Pulling pinned x86_64 Nitro image ${IMAGE}."
docker pull --platform linux/amd64 "$IMAGE"
image_arch="$(docker image inspect "$IMAGE" --format '{{.Architecture}}')"
[[ "$image_arch" == amd64 ]] || { echo "Expected Nitro image amd64, got ${image_arch}." >&2; exit 1; }
systemctl daemon-reload
systemctl enable docker.service arbitrum-snapshot-init.service arbitrum-nitro.service
systemctl start --no-block arbitrum-nitro.service
systemctl enable --now arbitrum-activate.timer
systemctl enable --now arbitrum-health.timer
echo "Bootstrap complete at $(date -Is); data UUID=${data_uuid}; Nitro=${image_arch}."
echo 'Without both parent-chain endpoints, Nitro prepares the snapshot only; live RPC/catch-up waits for endpoint configuration.'
