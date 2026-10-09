//! The fields a rule can read. One table drives everything: the compiler (names, types), the evaluator, the stored
//! row, and the catalogue the UIs build forms and autocomplete from.

use alloy::primitives::U256;
use mev_decode::{Decoded, PoolRef, calls::Leaf};
use mev_protocol::LiquidityAction;

use crate::{
    tx::Tx,
    value::{Ty, Val},
};

/// Everything a field can be read from: the transaction, the call (leaf) being judged, and its decoding.
pub struct Ctx<'a> {
    pub tx: &'a Tx,
    /// `None` only for a contract creation (no destination, so no call).
    pub leaf: Option<&'a Leaf>,
    pub decoded: Option<&'a Decoded>,
    /// Protocol id that owns the destination (or that produced the decoded intent).
    pub protocol: Option<&'static str>,
    /// The identified destination, if any.
    pub known: Option<&'static mev_decode::Contract>,
}

impl Ctx<'_> {
    /// `swap`, `liquidity`, `liquidation`, `undecoded_known` (an identified contract nothing reads yet) or `unknown`.
    pub fn kind(&self) -> &'static str {
        match self.decoded {
            Some(Decoded::Swap(_)) => "swap",
            Some(Decoded::Liquidity(_)) => "liquidity",
            Some(Decoded::Liquidation(_)) => "liquidation",
            None if self.known.is_some() => "undecoded_known",
            None => "unknown",
        }
    }
}

macro_rules! fields {
    ($( $id:ident, $name:literal, $ty:ident, $group:literal, $doc:literal; )*) => {
        /// One readable field.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum FieldId { $($id),* }

        impl FieldId {
            pub const ALL: &'static [FieldId] = &[$(FieldId::$id),*];
            pub fn name(self) -> &'static str { match self { $(FieldId::$id => $name),* } }
            pub fn ty(self) -> Ty { match self { $(FieldId::$id => Ty::$ty),* } }
            pub fn group(self) -> &'static str { match self { $(FieldId::$id => $group),* } }
            pub fn doc(self) -> &'static str { match self { $(FieldId::$id => $doc),* } }
        }
    };
}

fields! {
    // The transaction.
    Hash, "hash", Hash, "tx", "Transaction hash.";
    Seq, "seq", Num, "tx", "Sequencer message number.";
    From, "from", Addr, "tx", "Signer of the transaction.";
    TxTo, "tx_to", Addr, "tx", "Destination of the transaction itself (not of an inner call).";
    TxSelector, "tx_selector", Sel, "tx", "First four bytes of the transaction's calldata.";
    TxType, "tx_type", Num, "tx", "EIP-2718 type byte: 0 legacy, 1 access list, 2 EIP-1559, 3 blob, 4 set-code, Arbitrum 0x64..0x78.";
    Value, "value", Num, "tx", "ETH sent, in wei.";
    GasLimit, "gas_limit", Num, "tx", "Gas limit.";
    MaxFee, "max_fee", Num, "tx", "Max fee per gas, in wei.";
    Tip, "tip", Num, "tx", "Max priority fee per gas, in wei (unset for legacy transactions).";
    Nonce, "nonce", Num, "tx", "Sender nonce.";
    // The call being judged (a transaction can hold several, inside multicalls and bundles).
    To, "to", Addr, "call", "Destination of this call.";
    Selector, "selector", Sel, "call", "Function selector of this call.";
    Input, "input", Bytes, "call", "Calldata of this call (use `contains`).";
    Len, "len", Num, "call", "Calldata length in bytes.";
    Via, "via", StrList, "call", "Wrappers this call was found through: multicall3, self_multicall, account_execute, entrypoint_v7.";
    Depth, "depth", Num, "call", "How many wrappers deep (0 = called directly by the transaction).";
    Known, "known", Str, "call", "Name of the identified destination contract.";
    Protocol, "protocol", Str, "call", "Protocol id that owns the destination.";
    Kind, "kind", Str, "call", "swap, liquidity, liquidation, undecoded_known or unknown.";
    // A decoded swap.
    SwapPool, "swap.pool", Addr, "swap", "Pool the first hop targets (when the calldata names it by address or V4 id).";
    SwapTokenIn, "swap.token_in", Addr, "swap", "Token sold (zero address = native ETH on V4).";
    SwapAmountIn, "swap.amount_in", Num, "swap", "Exact input amount, in the token's smallest unit.";
    SwapHops, "swap.hops", Num, "swap", "Number of hops in the route.";
    SwapLimit, "swap.sqrt_limit", Num, "swap", "sqrtPriceLimitX96, when the call sets one.";
    // A decoded liquidity change.
    LiqAction, "liq.action", Str, "liquidity", "add or remove.";
    LiqPool, "liq.pool", Addr, "liquidity", "Pool, when the call names it.";
    LiqTokenId, "liq.token_id", Num, "liquidity", "Position NFT id.";
    LiqTickLower, "liq.tick_lower", Int, "liquidity", "Lower tick of the range.";
    LiqTickUpper, "liq.tick_upper", Int, "liquidity", "Upper tick of the range.";
    LiqAmount0, "liq.amount0", Num, "liquidity", "Token0 amount offered (add) or minimum accepted (remove).";
    LiqAmount1, "liq.amount1", Num, "liquidity", "Token1 amount offered (add) or minimum accepted (remove).";
    LiqLiquidity, "liq.liquidity", Num, "liquidity", "Liquidity units to remove.";
    // A decoded liquidation.
    LiquidationBorrower, "liquidation.borrower", Addr, "liquidation", "First targeted position.";
    LiquidationCollateral, "liquidation.collateral", Addr, "liquidation", "Collateral asset.";
    LiquidationDebt, "liquidation.debt", Addr, "liquidation", "Debt asset.";
    LiquidationAmount, "liquidation.amount", Num, "liquidation", "Debt to cover or assets to seize (units depend on the protocol).";
    LiquidationMarket, "liquidation.market", Hash, "liquidation", "Morpho market id.";
}

impl FieldId {
    pub fn from_name(name: &str) -> Option<FieldId> {
        Self::ALL.iter().copied().find(|f| f.name() == name)
    }

    /// The value, or `None` when this call has none (for example `swap.pool` on a liquidation).
    /// `Via` and `Input` are not scalar and are read by the evaluator directly.
    pub fn read<'a>(self, c: &Ctx<'a>) -> Option<Val<'a>> {
        use FieldId as F;
        let num = |n: u64| Some(Val::Num(U256::from(n)));
        let pool_addr = |p: &PoolRef| match p {
            PoolRef::Address(a) => Some(Val::Addr(*a)),
            _ => None,
        };
        match self {
            F::Hash => Some(Val::Hash(c.tx.hash)),
            F::Seq => num(c.tx.seq),
            F::From => c.tx.from.map(Val::Addr),
            F::TxTo => c.tx.to.map(Val::Addr),
            F::TxSelector => {
                c.tx.input
                    .get(..4)
                    .map(|s| Val::Sel([s[0], s[1], s[2], s[3]]))
            }
            F::TxType => num(u64::from(c.tx.tx_type)),
            F::Value => Some(Val::Num(c.tx.value)),
            F::GasLimit => num(c.tx.gas_limit),
            F::MaxFee => Some(Val::Num(U256::from(c.tx.max_fee))),
            F::Tip => c.tx.tip.map(|t| Val::Num(U256::from(t))),
            F::Nonce => num(c.tx.nonce),
            F::To => c.leaf.map(|l| Val::Addr(l.to)),
            F::Selector => c.leaf?.selector().map(Val::Sel),
            F::Len => c.leaf.map(|l| Val::Num(U256::from(l.input.len()))),
            F::Depth => c.leaf.map(|l| Val::Num(U256::from(l.via.len()))),
            F::Known => c.known.map(|k| Val::Str(k.name)),
            F::Protocol => c.protocol.map(Val::Str),
            F::Kind => Some(Val::Str(c.kind())),
            F::Via | F::Input => None,
            F::SwapPool => match c.decoded? {
                Decoded::Swap(s) => pool_addr(&s.pool),
                _ => None,
            },
            F::SwapTokenIn => match c.decoded? {
                Decoded::Swap(s) => Some(Val::Addr(s.token_in)),
                _ => None,
            },
            F::SwapAmountIn => match c.decoded? {
                Decoded::Swap(s) => Some(Val::Num(s.amount_in)),
                _ => None,
            },
            F::SwapHops => match c.decoded? {
                Decoded::Swap(s) => num(u64::from(s.hops)),
                _ => None,
            },
            F::SwapLimit => match c.decoded? {
                Decoded::Swap(s) => s.sqrt_limit_x96.map(Val::Num),
                _ => None,
            },
            F::LiqAction => match c.decoded? {
                Decoded::Liquidity(l) => Some(Val::Str(match l.action {
                    LiquidityAction::Add => "add",
                    LiquidityAction::Remove => "remove",
                })),
                _ => None,
            },
            F::LiqPool => match c.decoded? {
                Decoded::Liquidity(l) => l.pool.as_ref().and_then(pool_addr),
                _ => None,
            },
            F::LiqTokenId => match c.decoded? {
                Decoded::Liquidity(l) => l.token_id.map(Val::Num),
                _ => None,
            },
            F::LiqTickLower => match c.decoded? {
                Decoded::Liquidity(l) => l.tick_lower.map(|t| Val::Int(i64::from(t))),
                _ => None,
            },
            F::LiqTickUpper => match c.decoded? {
                Decoded::Liquidity(l) => l.tick_upper.map(|t| Val::Int(i64::from(t))),
                _ => None,
            },
            F::LiqAmount0 => match c.decoded? {
                Decoded::Liquidity(l) => l.amount0.map(Val::Num),
                _ => None,
            },
            F::LiqAmount1 => match c.decoded? {
                Decoded::Liquidity(l) => l.amount1.map(Val::Num),
                _ => None,
            },
            F::LiqLiquidity => match c.decoded? {
                Decoded::Liquidity(l) => l.liquidity.map(Val::Num),
                _ => None,
            },
            F::LiquidationBorrower => match c.decoded? {
                Decoded::Liquidation(l) => l.borrowers.first().copied().map(Val::Addr),
                _ => None,
            },
            F::LiquidationCollateral => match c.decoded? {
                Decoded::Liquidation(l) => l.collateral.map(Val::Addr),
                _ => None,
            },
            F::LiquidationDebt => match c.decoded? {
                Decoded::Liquidation(l) => l.debt.map(Val::Addr),
                _ => None,
            },
            F::LiquidationAmount => match c.decoded? {
                Decoded::Liquidation(l) => l.amount.map(Val::Num),
                _ => None,
            },
            F::LiquidationMarket => match c.decoded? {
                Decoded::Liquidation(l) => l.market.map(Val::Hash),
                _ => None,
            },
        }
    }
}
