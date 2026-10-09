//! What the engine knows, as data: fields, protocols, contracts, event kinds, wrappers. The UIs build their forms,
//! autocomplete and validation from this, so nothing about protocols is written into a UI.

use std::{collections::HashMap, sync::OnceLock};

use alloy::primitives::Address;
use mev_decode::protocols;
use serde::Serialize;

use crate::{field::FieldId, value::Ty};

/// Protocol ids in the registry (`uniswap_v3`, `aave_v3`, ...).
pub fn protocol_ids() -> Vec<&'static str> {
    protocols().iter().map(|p| p.id()).collect()
}

/// The protocol that owns each identified destination.
pub fn protocol_of(addr: Address) -> Option<&'static str> {
    static M: OnceLock<HashMap<Address, &'static str>> = OnceLock::new();
    M.get_or_init(|| {
        let mut m = HashMap::new();
        for p in protocols() {
            for c in p.contracts() {
                m.entry(c.address).or_insert(p.id());
            }
        }
        m
    })
    .get(&addr)
    .copied()
}

#[derive(Debug, Serialize)]
pub struct FieldInfo {
    pub name: &'static str,
    /// The field as a person names it.
    pub label: &'static str,
    pub ty: Ty,
    pub group: &'static str,
    pub doc: &'static str,
    /// The closed set of values, where the field has one.
    pub values: Option<Vec<&'static str>>,
}

#[derive(Debug, Serialize)]
pub struct ContractInfo {
    pub address: String,
    pub name: &'static str,
    /// Whether any call to it is decoded.
    pub decoded: bool,
}

#[derive(Debug, Serialize)]
pub struct EventInfo {
    pub topic0: String,
    pub name: &'static str,
    pub class: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ProtocolInfo {
    pub id: &'static str,
    pub label: String,
    pub contracts: Vec<ContractInfo>,
    pub events: Vec<EventInfo>,
}

#[derive(Debug, Serialize)]
pub struct TokenInfo {
    pub symbol: &'static str,
    pub address: String,
    pub decimals: u8,
}

#[derive(Debug, Serialize)]
pub struct Named {
    pub id: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Catalogue {
    /// Tokens people think in, with the decimals that turn a raw amount into a human one.
    pub tokens: Vec<TokenInfo>,
    /// Functions worth naming (selector without 0x, name).
    pub functions: Vec<(&'static str, &'static str)>,
    pub wrapper_labels: Vec<Named>,
    pub fields: Vec<FieldInfo>,
    pub protocols: Vec<ProtocolInfo>,
    pub wrappers: Vec<&'static str>,
    pub kinds: Vec<&'static str>,
    pub actions: Vec<&'static str>,
}

pub fn catalogue() -> Catalogue {
    let values = |f: FieldId| match f {
        FieldId::Kind => Some(vec![
            "swap",
            "liquidity",
            "liquidation",
            "undecoded_known",
            "unknown",
        ]),
        FieldId::LiqAction => Some(vec!["add", "remove"]),
        FieldId::Protocol => Some(protocol_ids()),
        FieldId::Via => Some(vec![
            "multicall3",
            "self_multicall",
            "account_execute",
            "entrypoint_v6",
            "entrypoint_v7",
        ]),
        _ => None,
    };
    Catalogue {
        tokens: crate::names::tokens()
            .into_iter()
            .map(|t| TokenInfo {
                symbol: t.symbol,
                address: format!("{:#x}", t.address),
                decimals: t.decimals,
            })
            .collect(),
        functions: crate::names::SELECTORS.to_vec(),
        wrapper_labels: [
            "multicall3",
            "self_multicall",
            "account_execute",
            "entrypoint_v6",
            "entrypoint_v7",
        ]
        .iter()
        .map(|w| Named {
            id: w,
            label: crate::names::wrapper_label(w),
        })
        .collect(),
        fields: FieldId::ALL
            .iter()
            .map(|f| FieldInfo {
                name: f.name(),
                label: crate::explain::label(*f),
                ty: f.ty(),
                group: f.group(),
                doc: f.doc(),
                values: values(*f),
            })
            .collect(),
        protocols: protocols()
            .iter()
            .map(|p| ProtocolInfo {
                id: p.id(),
                label: crate::names::protocol_label(p.id()).to_string(),
                contracts: p
                    .contracts()
                    .into_iter()
                    .map(|c| ContractInfo {
                        address: format!("{:#x}", c.address),
                        name: c.name,
                        decoded: c.decoded,
                    })
                    .collect(),
                events: p
                    .events()
                    .into_iter()
                    .map(|e| EventInfo {
                        topic0: format!("{:#x}", e.topic0),
                        name: e.name,
                        class: e.class.as_str(),
                    })
                    .collect(),
            })
            .collect(),
        wrappers: vec![
            "multicall3",
            "self_multicall",
            "account_execute",
            "entrypoint_v6",
            "entrypoint_v7",
        ],
        kinds: vec![
            "swap",
            "liquidity",
            "liquidation",
            "undecoded_known",
            "unknown",
        ],
        actions: vec!["store", "raw", "count", "drop", "alert"],
    }
}
