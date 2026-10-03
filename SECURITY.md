# Security

This repo controls a bot that signs transactions and an on-chain executor holding flash-loaned funds.

- Report issues privately to the repo owner directly — never in issues or PR comments.
- Hot-wallet keys live only in the runtime environment of the bot host — never in the repo, CI, or logs.
- Hot wallet holds gas only; profits are swept to a cold wallet daily.
- If a key may be exposed: pause the bot (kill switch in `mev-risk`), sweep funds, rotate the key, redeploy.
