//! Local HTTP JSON-RPC server used as a TEST FIXTURE (never production code): a scripted chain
//! with block hashes, a log store and failure injection (HTTP 429, per-request latency, forks).
//! Real clients (alloy over HTTP) talk to it over a real socket, so retry/backoff, error mapping
//! and reorg handling are exercised end to end.
#![allow(dead_code)]

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use alloy::primitives::{Address, B256, U256};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[derive(Default)]
pub struct NodeState {
    /// Canonical chain: height -> hash.
    pub blocks: BTreeMap<u64, B256>,
    /// Raw RPC logs on the canonical chain (`blockNumber`, `blockHash`, ... set by the test).
    pub logs: Vec<Value>,
    /// The next N requests answer HTTP 429.
    pub rate_limit_next: usize,
    /// The next N `eth_getLogs` requests answer HTTP 429.
    pub rate_limit_logs: usize,
    /// `eth_getLogs` ranges wider than this answer a provider-limit error.
    pub max_log_range: Option<u64>,
    /// Every `eth_getLogs` range requested, in order (including failed ones).
    pub log_requests: Vec<(u64, u64)>,
    /// Count of requests answered 429.
    pub rate_limited: usize,
    /// All `eth_getLogs` calls fail with this error message (after rate limits).
    pub fail_logs: Option<String>,
    /// `eth_call` results by `(to, selector)`: 0x-prefixed ABI-encoded return data.
    pub calls: BTreeMap<(Address, [u8; 4]), String>,
    /// `eth_getStorageAt` results by `(address, slot)`.
    pub storage: BTreeMap<(Address, U256), B256>,
    /// `eth_getCode` results.
    pub code: BTreeMap<Address, String>,
}

pub struct FakeNode {
    pub url: String,
    pub state: Arc<Mutex<NodeState>>,
}

pub fn hash_of(fork: u8, number: u64) -> B256 {
    let mut b = [0u8; 32];
    b[0] = fork;
    b[24..].copy_from_slice(&number.to_be_bytes());
    B256::from(b)
}

fn hex(n: u64) -> String {
    format!("0x{n:x}")
}

fn parse_u64(v: &Value, head: u64) -> u64 {
    match v.as_str() {
        Some("latest") | Some("pending") | Some("safe") | Some("finalized") | None => head,
        Some("earliest") => 0,
        Some(s) => u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(head),
    }
}

fn block_json(number: u64, hash: B256, parent: B256) -> Value {
    let z32 = format!("{:#x}", B256::ZERO);
    json!({
        "hash": format!("{hash:#x}"),
        "parentHash": format!("{parent:#x}"),
        "sha3Uncles": z32, "miner": format!("{:#x}", Address::ZERO),
        "stateRoot": z32, "transactionsRoot": z32, "receiptsRoot": z32,
        "logsBloom": format!("0x{}", "00".repeat(256)),
        "difficulty": "0x1", "number": hex(number),
        "gasLimit": "0x1c9c380", "gasUsed": "0x0", "timestamp": hex(1_700_000_000 + number),
        "extraData": "0x", "mixHash": z32, "nonce": "0x0000000000000000",
        "baseFeePerGas": "0x1", "size": "0x200",
        "transactions": [], "uncles": []
    })
}

fn matches_filter(log: &Value, f: &Value, from: u64, to: u64) -> bool {
    let n = u64::from_str_radix(
        log["blockNumber"]
            .as_str()
            .unwrap_or("0x0")
            .trim_start_matches("0x"),
        16,
    )
    .unwrap_or(0);
    if n < from || n > to {
        return false;
    }
    let addr_ok = match &f["address"] {
        Value::Null => true,
        Value::String(a) => a.eq_ignore_ascii_case(log["address"].as_str().unwrap_or("")),
        Value::Array(v) => {
            v.is_empty()
                || v.iter().any(|a| {
                    a.as_str().is_some_and(|a| {
                        a.eq_ignore_ascii_case(log["address"].as_str().unwrap_or(""))
                    })
                })
        }
        _ => false,
    };
    if !addr_ok {
        return false;
    }
    let topics = f["topics"].as_array().cloned().unwrap_or_default();
    for (i, want) in topics.iter().enumerate() {
        let have = log["topics"][i].as_str().unwrap_or("");
        let ok = match want {
            Value::Null => true,
            Value::String(w) => w.eq_ignore_ascii_case(have),
            Value::Array(ws) => {
                ws.is_empty()
                    || ws
                        .iter()
                        .any(|w| w.as_str().is_some_and(|w| w.eq_ignore_ascii_case(have)))
            }
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

fn answer(st: &mut NodeState, req: &Value) -> Value {
    let id = req["id"].clone();
    let head = st.blocks.keys().next_back().copied().unwrap_or(0);
    let p = &req["params"];
    let result = match req["method"].as_str().unwrap_or("") {
        "eth_blockNumber" => json!(hex(head)),
        "eth_chainId" => json!("0xa4b1"),
        "eth_getBlockByNumber" => {
            let n = parse_u64(&p[0], head);
            match st.blocks.get(&n) {
                Some(h) => {
                    let parent = st
                        .blocks
                        .get(&n.wrapping_sub(1))
                        .copied()
                        .unwrap_or_default();
                    block_json(n, *h, parent)
                }
                None => Value::Null,
            }
        }
        "eth_getLogs" => {
            let f = &p[0];
            let from = parse_u64(&f["fromBlock"], head);
            let to = parse_u64(&f["toBlock"], head);
            st.log_requests.push((from, to));
            if let Some(m) = st.max_log_range
                && to - from + 1 > m
            {
                return json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":format!("query spans {} blocks, only {m} allowed; narrow the block range", to - from + 1)}});
            }
            if let Some(msg) = &st.fail_logs {
                return json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":msg}});
            }
            let logs: Vec<&Value> = st
                .logs
                .iter()
                .filter(|l| matches_filter(l, f, from, to))
                .collect();
            json!(logs)
        }
        "eth_call" => {
            let to: Address = p[0]["to"]
                .as_str()
                .and_then(|a| a.parse().ok())
                .unwrap_or_default();
            let data = p[0]["input"]
                .as_str()
                .or(p[0]["data"].as_str())
                .unwrap_or("0x");
            let raw = alloy::hex::decode(data).unwrap_or_default();
            let mut sel = [0u8; 4];
            sel.copy_from_slice(&[raw.as_slice(), &[0; 4]].concat()[..4]);
            match st.calls.get(&(to, sel)) {
                Some(r) => json!(r),
                None => {
                    return json!({"jsonrpc":"2.0","id":id,"error":{"code":3,"message":"execution reverted"}});
                }
            }
        }
        "eth_getStorageAt" => {
            let a: Address = p[0]
                .as_str()
                .and_then(|a| a.parse().ok())
                .unwrap_or_default();
            let slot =
                U256::from_str_radix(p[1].as_str().unwrap_or("0x0").trim_start_matches("0x"), 16)
                    .unwrap_or_default();
            json!(format!(
                "{:#x}",
                st.storage.get(&(a, slot)).copied().unwrap_or_default()
            ))
        }
        "eth_getCode" => {
            let a: Address = p[0]
                .as_str()
                .and_then(|a| a.parse().ok())
                .unwrap_or_default();
            json!(st.code.get(&a).cloned().unwrap_or_else(|| "0x".to_string()))
        }
        other => {
            return json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":format!("unsupported {other}")}});
        }
    };
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

impl FakeNode {
    pub async fn start(state: NodeState) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(state));
        let shared = state.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let st = shared.clone();
                tokio::spawn(async move {
                    let mut buf: Vec<u8> = vec![];
                    let mut chunk = [0u8; 8192];
                    loop {
                        // Read one HTTP request (headers + Content-Length body).
                        let (head_end, len) = loop {
                            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                let head = String::from_utf8_lossy(&buf[..i]).to_lowercase();
                                let len = head
                                    .lines()
                                    .find_map(|l| l.strip_prefix("content-length:"))
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                                    .unwrap_or(0);
                                break (i + 4, len);
                            }
                            match sock.read(&mut chunk).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                            }
                        };
                        while buf.len() < head_end + len {
                            match sock.read(&mut chunk).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                            }
                        }
                        let body: Vec<u8> = buf[head_end..head_end + len].to_vec();
                        buf.drain(..head_end + len);
                        let (status, out) = {
                            let mut s = st.lock().unwrap();
                            let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                            let is_logs = req["method"] == "eth_getLogs";
                            if s.rate_limit_next > 0 || (is_logs && s.rate_limit_logs > 0) {
                                if s.rate_limit_next > 0 {
                                    s.rate_limit_next -= 1;
                                } else {
                                    s.rate_limit_logs -= 1;
                                }
                                s.rate_limited += 1;
                                (
                                    "429 Too Many Requests",
                                    "{\"error\":\"rate limit exceeded\"}".to_string(),
                                )
                            } else {
                                let resp = match &req {
                                    Value::Array(items) => Value::Array(
                                        items.iter().map(|r| answer(&mut s, r)).collect(),
                                    ),
                                    r => answer(&mut s, r),
                                };
                                ("200 OK", resp.to_string())
                            }
                        };
                        let msg = format!(
                            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: keep-alive\r\n\r\n{out}",
                            out.len()
                        );
                        if sock.write_all(msg.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        Self { url, state }
    }

    /// Replace the canonical chain from `from` upward with `fork` hashes up to `head`, dropping
    /// logs of replaced blocks and adding `new_logs`.
    pub fn reorg(&self, from: u64, head: u64, fork: u8, new_logs: Vec<Value>) {
        let mut s = self.state.lock().unwrap();
        s.blocks.retain(|n, _| *n < from);
        for n in from..=head {
            s.blocks.insert(n, hash_of(fork, n));
        }
        s.logs.retain(|l| {
            u64::from_str_radix(
                l["blockNumber"]
                    .as_str()
                    .unwrap_or("0x0")
                    .trim_start_matches("0x"),
                16,
            )
            .unwrap_or(0)
                < from
        });
        s.logs.extend(new_logs);
    }
}

pub fn chain(fork: u8, from: u64, to: u64) -> BTreeMap<u64, B256> {
    (from..=to).map(|n| (n, hash_of(fork, n))).collect()
}
