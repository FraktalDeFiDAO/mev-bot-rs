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
    pub contracts: Vec<ContractInfo>,
    pub events: Vec<EventInfo>,
}

#[derive(Debug, Serialize)]
pub struct Catalogue {
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
            "entrypoint_v7",
        ]),
        _ => None,
    };
    Catalogue {
        fields: FieldId::ALL
            .iter()
            .map(|f| FieldInfo {
                name: f.name(),
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
