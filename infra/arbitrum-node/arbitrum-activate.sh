#!/usr/bin/env bash
set -Eeuo pipefail
[[ -f /data/arbitrum/snapshot-import-complete.json ]] || exit 0
# Do not interrupt a live node or the offline post-import DB validation.
if systemctl is-active --quiet arbitrum-nitro.service; then exit 0; fi
token="$(curl -fsS --max-time 5 -X PUT -H 'X-aws-ec2-metadata-token-ttl-seconds: 21600' http://169.254.169.254/latest/api/token)"
region="$(curl -fsS --max-time 5 -H "X-aws-ec2-metadata-token: $token" http://169.254.169.254/latest/meta-data/placement/region)"
export ETHEREUM_RPC_URL ETHEREUM_BEACON_URL
ETHEREUM_RPC_URL="$(aws ssm get-parameter --region "$region" --name /mev-bot/arbitrum/ethereum-rpc-url --with-decryption --query Parameter.Value --output text 2>/dev/null || true)"
ETHEREUM_BEACON_URL="$(aws ssm get-parameter --region "$region" --name /mev-bot/arbitrum/ethereum-beacon-url --with-decryption --query Parameter.Value --output text 2>/dev/null || true)"
[[ -n "$ETHEREUM_RPC_URL" && -n "$ETHEREUM_BEACON_URL" ]] || exit 0
/usr/local/sbin/arbitrum-parent-ready || exit 0
echo 'Verified Ethereum mainnet parent head and snapshot-era blobs. Starting normal Nitro with L1 validation.'
systemctl reset-failed arbitrum-nitro.service
systemctl start --no-block arbitrum-nitro.service
