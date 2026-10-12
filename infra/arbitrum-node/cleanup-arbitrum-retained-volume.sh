#!/usr/bin/env bash
# Never invoked automatically. Deletes ONLY the known retired migration disk.
set -Eeuo pipefail
export AWS_PAGER=''
PROFILE="${AWS_PROFILE:-mev-bot}"; REGION="${AWS_REGION:-us-east-2}"
OLD=vol-0e85eb2e15297fafe; NEW=vol-0c04188add3a6a538; INSTANCE=i-0245433f99336c72b
DELETE=false; CONFIRM=''
while (( $# )); do
  case "$1" in
    --delete-data-volume) DELETE=true; shift;;
    --confirm-volume-id) CONFIRM="$2"; shift 2;;
    --profile) PROFILE="$2"; shift 2;; --region) REGION="$2"; shift 2;;
    --help) echo 'Requires --delete-data-volume --confirm-volume-id vol-0e85eb2e15297fafe; refuses until the new node is synced and validated.'; exit 0;;
    *) echo "Unknown option: $1" >&2; exit 2;;
  esac
done
[[ "$DELETE" == true && "$CONFIRM" == "$OLD" ]] || { echo 'No action: exact destructive flag and retired volume confirmation are required.' >&2; exit 2; }
awsr() { aws --profile "$PROFILE" --region "$REGION" "$@"; }
[[ "$(awsr sts get-caller-identity --query Account --output text)" == 659648171648 ]] || exit 1
volume="$(awsr ec2 describe-volumes --volume-ids "$OLD" --output json)"
jq -e --arg new "$NEW" --arg instance "$INSTANCE" '.Volumes[0] |
 .Encrypted == true and .Size == 6144 and
 (.Tags | any(.Key=="Name" and .Value=="arbitrum-one-node-retained-snapshot-cache")) and
 (.Tags | any(.Key=="ManagedBy" and .Value=="aws-cli")) and
 (.Tags | any(.Key=="ReplacedBy" and .Value==$new)) and
 (.Attachments | length==1 and .[0].InstanceId==$instance)' >/dev/null <<<"$volume" || { echo 'Retired disk ownership/attachment mismatch; preserved.' >&2; exit 1; }
echo "EXPLICIT DESTRUCTIVE ACTION: delete retired 6144 GiB disk ${OLD} in ${REGION}, only after validating ${NEW}. Removes about \$591.52/month of EBS charges."
tmp="$(mktemp -d)"; trap 'rm -rf -- "$tmp"' EXIT
cat >"$tmp/unmount.sh" <<'SH'
#!/bin/bash
set -Eeuo pipefail
test -f /data/arbitrum/snapshot-import-complete.json
test "$(findmnt -n -o UUID /data/arbitrum)" = d2c32828-7496-4eef-a162-80b45f380f20
rpc() { curl -fsS --max-time 10 -H 'Content-Type: application/json' --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":[]}" http://127.0.0.1:8547; }
test "$(rpc eth_chainId | jq -r '.result')" = 0xa4b1
test "$(rpc eth_syncing | jq -c '.result')" = false
arbitrum-status --validate 120
local_head="$(rpc eth_blockNumber | jq -r '.result')"
reference="$(curl -fsS --max-time 10 -H 'Content-Type: application/json' --data '{"jsonrpc":"2.0","id":1,"method":"eth_blockNumber","params":[]}' https://arb1.arbitrum.io/rpc | jq -r '.result')"
[[ "$local_head" =~ ^0x[0-9a-fA-F]+$ && "$reference" =~ ^0x[0-9a-fA-F]+$ ]]
(( reference - local_head <= 100 ))
test "$(findmnt -n -o UUID /data/arbitrum-old)" = ddfcc335-56b5-4eb1-ac70-e1ecbd89c5f0
systemctl stop arbitrum-snapshot-prefetch.service 2>/dev/null || true
sync -f /data/arbitrum-old
umount /data/arbitrum-old
python3 - <<'PY'
from pathlib import Path
p=Path('/etc/fstab'); rows=p.read_text().splitlines(); out=[]
for row in rows:
    fields=row.split()
    if len(fields)>1 and fields[0]=='UUID=ddfcc335-56b5-4eb1-ac70-e1ecbd89c5f0' and fields[1]=='/data/arbitrum-old':
        continue
    out.append(row)
temporary=Path('/etc/fstab.retired-disk-cleanup');temporary.write_text('\n'.join(out)+'\n');temporary.chmod(0o644);temporary.replace(p)
PY
systemctl daemon-reload
mount -a
mountpoint -q /data/arbitrum
SH
jq -n --rawfile command "$tmp/unmount.sh" '{commands:[$command],executionTimeout:["600"]}' >"$tmp/ssm.json"
id="$(awsr ssm send-command --instance-ids "$INSTANCE" --document-name AWS-RunShellScript --parameters "file://${tmp}/ssm.json" --query Command.CommandId --output text)"
status=''
for _ in $(seq 1 120); do
  status="$(awsr ssm get-command-invocation --command-id "$id" --instance-id "$INSTANCE" --query Status --output text 2>/dev/null || true)"
  [[ "$status" != Success ]] || break
  [[ "$status" != Failed && "$status" != TimedOut && "$status" != Cancelled ]] || { echo "Remote validation failed (${status}); retired disk preserved. SSM command=${id}" >&2; exit 1; }
  sleep 5
done
[[ "$status" == Success ]] || { echo 'Validation timed out; retired disk preserved.' >&2; exit 1; }
awsr ec2 detach-volume --volume-id "$OLD" --instance-id "$INSTANCE"
awsr ec2 wait volume-available --volume-ids "$OLD"
awsr ec2 delete-volume --volume-id "$OLD"
echo "Deleted explicitly confirmed retired disk ${OLD}; active ${NEW} preserved."
