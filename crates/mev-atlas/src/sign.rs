//! SolverOperation and its EIP-712 signature (domain `AtlasVerification` v1.6.4).

use alloy::{
    primitives::{Address, B256, Bytes, U256},
    signers::{SignerSync, local::PrivateKeySigner},
    sol,
    sol_types::{Eip712Domain, SolStruct},
};
use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::{ATLAS_VERIFICATION, ATLAS_VERSION};

sol! {
    /// Field order and types as in Atlas's `SolverOperation` (without `signature`); the generated
    /// EIP-712 type string must equal `SOLVER_TYPEHASH`'s preimage (tested).
    #[derive(Debug, PartialEq)]
    struct SolverOperation {
        address from;
        address to;
        uint256 value;
        uint256 gas;
        uint256 maxFeePerGas;
        uint256 deadline;
        address solver;
        address control;
        bytes32 userOpHash;
        address bidToken;
        uint256 bidAmount;
        bytes data;
    }
}

/// `AtlasVerification`'s EIP-712 domain on `chain_id`.
pub fn domain(chain_id: u64) -> Eip712Domain {
    Eip712Domain::new(
        Some("AtlasVerification".into()),
        Some(ATLAS_VERSION.into()),
        Some(U256::from(chain_id)),
        Some(ATLAS_VERIFICATION),
        None,
    )
}

/// The digest a solver signs (what `AtlasVerification.getSolverPayload` returns).
pub fn digest(op: &SolverOperation, chain_id: u64) -> B256 {
    op.eip712_signing_hash(&domain(chain_id))
}

#[derive(Debug, Clone)]
pub struct SignedSolverOp {
    pub op: SolverOperation,
    pub signature: Bytes,
}

impl SignedSolverOp {
    /// The `auction_solution` object of `solver_submitSolverOperation` (hex quantities).
    pub fn to_json(&self) -> Value {
        let q = |v: U256| format!("{v:#x}");
        let o = &self.op;
        json!({
            "from": o.from, "to": o.to, "value": q(o.value), "gas": q(o.gas),
            "maxFeePerGas": q(o.maxFeePerGas), "deadline": q(o.deadline),
            "solver": o.solver, "control": o.control, "userOpHash": o.userOpHash,
            "bidToken": o.bidToken, "bidAmount": q(o.bidAmount), "data": o.data,
            "signature": self.signature,
        })
    }
}

/// Sign `op` with the bonded EOA; `op.from` must be the signer's address.
pub fn sign(
    op: SolverOperation,
    signer: &PrivateKeySigner,
    chain_id: u64,
) -> Result<SignedSolverOp> {
    anyhow::ensure!(
        op.from == signer.address(),
        "solver operation `from` is not the signer"
    );
    let sig = signer
        .sign_hash_sync(&digest(&op, chain_id))
        .context("signing failed")?;
    Ok(SignedSolverOp {
        op,
        signature: Bytes::from(sig.as_bytes().to_vec()),
    })
}

/// The auction-compliant operation for `auction`: Atlas as `to`, the oracle's exact gas price and
/// the auction's control/user-op hash. The caller supplies the bid, gas limit and call data.
pub struct OpParams<'a> {
    pub auction: &'a crate::Auction,
    pub from: Address,
    pub solver: Address,
    pub gas: U256,
    pub bid_amount: U256,
    pub data: Bytes,
}

pub fn build(p: OpParams<'_>) -> SolverOperation {
    SolverOperation {
        from: p.from,
        to: p.auction.to,
        value: U256::ZERO,
        gas: p.gas,
        maxFeePerGas: p.auction.max_fee_per_gas,
        deadline: p.auction.deadline,
        solver: p.solver,
        control: p.auction.control,
        userOpHash: p.auction.user_op_hash,
        bidToken: Address::ZERO, // native ETH
        bidAmount: p.bid_amount,
        data: p.data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        primitives::{address, keccak256},
        sol_types::SolStruct,
    };

    fn op(from: Address) -> SolverOperation {
        SolverOperation {
            from,
            to: crate::ATLAS,
            value: U256::ZERO,
            gas: U256::from(500_000u64),
            maxFeePerGas: U256::from(39_820_200u64),
            deadline: U256::from(100u64),
            solver: address!("00000000000000000000000000000000000000aa"),
            control: address!("e15bba987c002ecc3586e81244517877d294d291"),
            userOpHash: B256::repeat_byte(0x11),
            bidToken: Address::ZERO,
            bidAmount: U256::from(1_000_000_000_000u64),
            data: Bytes::from_static(&[1, 2, 3]),
        }
    }

    #[test]
    fn eip712_type_matches_atlas_solver_typehash() {
        let atlas = "SolverOperation(address from,address to,uint256 value,uint256 gas,uint256 maxFeePerGas,uint256 deadline,address solver,address control,bytes32 userOpHash,address bidToken,uint256 bidAmount,bytes data)";
        assert_eq!(SolverOperation::eip712_encode_type(), atlas);
        assert_eq!(op(Address::ZERO).eip712_type_hash(), keccak256(atlas));
    }

    #[test]
    fn signature_recovers_to_the_bonded_signer() {
        let signer = PrivateKeySigner::random();
        let s = sign(op(signer.address()), &signer, 42161).unwrap();
        let sig = alloy::primitives::Signature::from_raw(&s.signature).unwrap();
        let rec = sig
            .recover_address_from_prehash(&digest(&s.op, 42161))
            .unwrap();
        assert_eq!(rec, signer.address());
        // A different chain id changes the digest (replay protection).
        assert_ne!(digest(&s.op, 42161), digest(&s.op, 1));
    }

    #[test]
    fn refuses_to_sign_for_a_different_from_address() {
        let signer = PrivateKeySigner::random();
        assert!(sign(op(Address::ZERO), &signer, 42161).is_err());
    }

    #[test]
    fn build_follows_the_auction_rules() {
        let a = crate::parse_notification(&crate::tests::notification())
            .unwrap()
            .unwrap();
        let o = build(OpParams {
            auction: &a,
            from: Address::repeat_byte(1),
            solver: Address::repeat_byte(2),
            gas: U256::from(400_000u64),
            bid_amount: U256::from(7u64),
            data: Bytes::new(),
        });
        assert_eq!(
            o.maxFeePerGas, a.max_fee_per_gas,
            "gas price exactly the oracle's"
        );
        assert_eq!(o.to, a.to);
        assert_eq!(o.userOpHash, a.user_op_hash);
        assert_eq!(o.bidToken, Address::ZERO);
        assert_eq!(o.value, U256::ZERO);
    }

    #[test]
    fn json_uses_hex_quantities() {
        let signer = PrivateKeySigner::random();
        let s = sign(op(signer.address()), &signer, 42161).unwrap();
        let j = s.to_json();
        assert_eq!(j["gas"], "0x7a120");
        assert_eq!(j["maxFeePerGas"], format!("{:#x}", 39_820_200u64));
        assert_eq!(j["bidAmount"], format!("{:#x}", 1_000_000_000_000u64));
    }
}
