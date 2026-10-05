# Arbitrum One Nitro node

This deployment is a single x86_64 EC2 full/pruned node using Nitro's default
HashDB scheme. It is not an archive node. Nitro is pinned to
`offchainlabs/nitro-node:v3.12.1-70fa99a`; the static configuration is in
[`nitro-config.json`](nitro-config.json), and parent-chain values are supplied
through the root-only `/etc/arbitrum/nitro.env` or the scoped SSM parameters.

## Parent-chain inputs

Both values are required before the service can start. The local provisioning
script can load `ETHEREUM_RPC_URL` and `ETHEREUM_BEACON_URL` from its process
environment into new SecureString parameters without putting their values in
user data. It preserves existing parameters; the instance can also read these
parameter names directly:

- `NITRO_PARENT__CHAIN_CONNECTION_URL`: Ethereum execution JSON-RPC endpoint.
- `NITRO_PARENT__CHAIN_BLOB__CLIENT_BEACON__URL`: Ethereum beacon API endpoint
  with the historical blob sidecars Nitro needs.

Use an SSM `SecureString` at `/mev-bot/arbitrum/ethereum-rpc-url` and
`/mev-bot/arbitrum/ethereum-beacon-url`, or populate both variables in
`/etc/arbitrum/nitro.env` and keep that file mode `0600`. Nitro reads them via
its `NITRO_` environment prefix. The instance role can read only parameters
under `/mev-bot/arbitrum/`; it has no general AWS permissions.

After supplying the URLs, start or restart the service and validate the node:

```bash
sudo systemctl restart arbitrum-nitro
sudo arbitrum-status
sudo arbitrum-status --validate 1800
```

The validation helper checks chain ID `0xa4b1`, HTTP RPC, `eth_syncing`, local
head progress, and distance from `ARBITRUM_REFERENCE_RPC_URL` when reachable.
The reference endpoint defaults to the public Arbitrum One RPC and is only a
comparison source; it is not used for Ethereum parent-chain reads.

## RPC and transaction forwarding

Nitro binds inside the container, while Docker publishes HTTP and WebSocket RPC
only on host loopback (`127.0.0.1:8547` and `127.0.0.1:8548`). Security-group
ingress does not include RPC, WebSocket, feed, or Docker ports. The node uses
the official Arbitrum One feed URL `wss://arb1.arbitrum.io/feed` and the
documented public endpoint `https://arb1.arbitrum.io/rpc` as transaction
forwarding target. That forwarding target may have public-service rate limits;
replace it with a supported provider if submission volume requires one.

`debug` is enabled only on loopback HTTP and WebSocket endpoints for local
tracing. The default Nitro
transaction lookup window remains in effect; older historical lookups beyond
that window need an archive-capable provider.

## Storage and service operations

The 4 TiB encrypted gp3 data volume is mounted by filesystem UUID at
`/data/arbitrum`; Nitro's snapshot download file and database both live there.
The bootstrap formats only a blank device and preserves any detected existing
filesystem. The data volume has `DeleteOnTermination=false`. Root uses encrypted
50 GiB gp3 and is deleted with the instance.

`arbitrum-nitro.service` supervises the foreground Docker process. It waits for
network and the data mount, restarts on failure, rotates Docker logs, and allows
30 minutes for graceful shutdown. `systemctl enable` is set. The conservative
host tuning is recorded in `/etc/sysctl.d/99-arbitrum.conf` (swappiness 10) and
`/etc/systemd/journald.conf.d/arbitrum.conf` (1 GiB persistent journal cap).

For a controlled Nitro version change: update `IMAGE` in `nitro-start.sh`,
update the pinned image reference in the bootstrap script and this document,
rebuild/redeploy the config bundle, then restart the service. Do not change
HashDB to PathDB on the existing database; PathDB needs a compatible
`full-path` snapshot and a fresh initialization.

## Estimated monthly cost

At current us-east-1 list rates, the target `r7i.2xlarge` is about $386/month
for 730 hours. The 4,096 GiB gp3 data volume at 16,000 IOPS and 1,000 MiB/s is
about $428/month; 50 GiB gp3 root is about $4/month. A public IPv4 adds roughly
$3.65/month if assigned. Expected subtotal is about **$822/month**, before
Internet data transfer, snapshots/backups, taxes, or other account usage.

## Resource state and teardown

`provision-arbitrum-node.sh` writes resource IDs to
`arbitrum-node-resources.env` next to the scripts (git-ignored); `teardown-arbitrum-node.sh` reads it
from the same place (override either with `ARBITRUM_STATE_FILE`). Run both as
`infra/arbitrum-node/<script>` from the repo root. Default teardown terminates only the instance
and preserves the data volume. Deleting the blockchain database requires both
`--delete-data-volume` and `--confirm-volume-id <volume-id>`, followed by typing
the volume ID confirmation phrase.
