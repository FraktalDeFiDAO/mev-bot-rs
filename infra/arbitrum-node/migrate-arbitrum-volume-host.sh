#!/usr/bin/env bash
# Run as root on the existing dedicated node after attaching the tagged 4 TiB replacement.
set -Eeuo pipefail
[[ $EUID == 0 ]] || { echo 'Run as root'; exit 1; }
old_volume=vol-0e85eb2e15297fafe
new_volume=vol-0c04188add3a6a538
old_uuid=ddfcc335-56b5-4eb1-ac70-e1ecbd89c5f0
bundle=/opt/arbitrum-volume-migration
mountpoint -q /data/arbitrum || { echo 'Current data mount is missing'; exit 1; }
new_device="$(lsblk -dn -o NAME,SERIAL | awk -v serial="${new_volume//-/}" '$2 == serial {print "/dev/"$1}')"
[[ -b "$new_device" ]] || { echo 'Replacement EBS serial not found'; exit 1; }
[[ "$(blockdev --getsize64 "$new_device")" == 4398046511104 ]] || { echo 'Replacement is not exactly 4 TiB'; exit 1; }
fs="$(blkid -s TYPE -o value "$new_device" || true)"
if [[ -z "$fs" ]]; then
  [[ -z "$(wipefs --no-act --noheadings --output TYPE "$new_device")" ]] || { echo 'Replacement has disk signatures; refusing format'; exit 1; }
  mkfs.xfs -L arbitrum4 "$new_device"
else
  [[ "$fs" == xfs ]] || { echo 'Unexpected replacement filesystem'; exit 1; }
fi
new_uuid="$(blkid -s UUID -o value "$new_device")"
if [[ "$(findmnt -n -o UUID /data/arbitrum)" == "$old_uuid" ]]; then
  # Hardlinks preserve downloaded bytes despite Nitro's tmp cleanup on shutdown.
  install -d -o 1000 -g 1000 -m 0750 /data/arbitrum/snapshot-cache
  for part in /data/arbitrum/nitro/tmp/pruned.tar.part*; do
    [[ -f "$part" ]] || continue
    cache="/data/arbitrum/snapshot-cache/${part##*/}"
    if [[ -e "$cache" ]]; then [[ "$cache" -ef "$part" ]] || { echo 'Conflicting cached snapshot part'; exit 1; }
    else ln "$part" "$cache"; fi
  done
  systemctl stop arbitrum-nitro.service
  [[ -z "$(docker ps -q --filter name=arbitrum-nitro)" ]] || { echo 'Nitro has not stopped'; exit 1; }
  sync -f /data/arbitrum
  cp -an /etc/fstab /etc/fstab.before-arbitrum-4tib
  install -d -m 0755 /data/arbitrum-old
  umount /data/arbitrum
  python3 - "$old_uuid" "$new_uuid" <<'PY'
import sys
from pathlib import Path
p=Path('/etc/fstab'); old,new=sys.argv[1:]
rows=p.read_text().splitlines(); out=[]; found=False
for row in rows:
    fields=row.split()
    if fields and fields[0] == 'UUID='+old and len(fields)>1 and fields[1] == '/data/arbitrum':
        fields[1]='/data/arbitrum-old'; row=' '.join(fields); found=True
    out.append(row)
if not found: raise SystemExit('Original UUID fstab entry not found')
if not any(len(r.split())>1 and r.split()[1]=='/data/arbitrum' for r in out):
    out.append(f'UUID={new} /data/arbitrum xfs defaults,noatime,nofail,x-systemd.device-timeout=120s 0 2')
tmp=Path('/etc/fstab.arbitrum-new'); tmp.write_text('\n'.join(out)+'\n'); tmp.chmod(0o644); tmp.replace(p)
PY
  systemctl daemon-reload
  mount -a
fi
[[ "$(findmnt -n -o UUID /data/arbitrum)" == "$new_uuid" ]] || { echo 'Replacement mount UUID mismatch'; exit 1; }
[[ "$(findmnt -n -o UUID /data/arbitrum-old)" == "$old_uuid" ]] || { echo 'Preserved old mount UUID mismatch'; exit 1; }
chown 1000:1000 /data/arbitrum
chmod 0750 /data/arbitrum
install -m 0755 "$bundle/arbitrum-snapshot-init.py" /usr/local/sbin/arbitrum-snapshot-init
install -m 0644 "$bundle/arbitrum-snapshot-init.service" /etc/systemd/system/arbitrum-snapshot-init.service
install -m 0644 "$bundle/nitro-node.service" /etc/systemd/system/arbitrum-nitro.service
install -m 0644 "$bundle/nitro-config.json" /etc/arbitrum/nitro-config.json
install -m 0755 "$bundle/nitro-start.sh" /usr/local/sbin/arbitrum-nitro-run
install -m 0755 "$bundle/arbitrum-status" /usr/local/bin/arbitrum-status
printf 'ARBITRUM_SNAPSHOT_URL=https://snapshot.arbitrum.foundation/arb1/2026-09-23-141f8381/\n' >/etc/arbitrum/snapshot.env
chmod 0644 /etc/arbitrum/snapshot.env
systemctl daemon-reload
systemctl enable arbitrum-snapshot-init.service arbitrum-nitro.service
systemctl start --no-block arbitrum-nitro.service
echo "Migration started: old=${old_volume} retained; new=${new_volume}; UUID=${new_uuid}."
findmnt /data/arbitrum /data/arbitrum-old || true
