//! Human names for things that otherwise show as hex: tokens, protocols, wrappers, well-known functions.

use alloy::primitives::Address;

/// Tokens people think in, with the decimals that turn a raw amount into a human one. Arbitrum One addresses.
pub const TOKENS: [(&str, &str, u8); 8] = [
    ("USDC", "af88d065e77c8cC2239327C5EDb3A432268e5831", 6),
    ("USDC.e", "FF970A61A04b1cA14834A43f5dE4533eBDDB5CC8", 6),
    ("USDT", "Fd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9", 6),
    ("WETH", "82aF49447D8a07e3bd95BD0d56f35241523fBab1", 18),
    ("WBTC", "2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f", 8),
    ("ARB", "912CE59144191C1204E64559FE8253a0e49E6548", 18),
    ("DAI", "DA10009cBd5D07dd0CeCc66161FC93D7c9000da1", 18),
    ("GMX", "fc5A1A6EB076a2C7aD06eD22C90d7E710E35ad0a", 18),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub symbol: &'static str,
    pub address: Address,
    pub decimals: u8,
}

pub fn tokens() -> Vec<Token> {
    TOKENS
        .iter()
        .map(|(symbol, a, decimals)| Token {
            symbol,
            address: a.parse().expect("token address"),
            decimals: *decimals,
        })
        .collect()
}

pub fn token_by_address(a: Address) -> Option<Token> {
    tokens().into_iter().find(|t| t.address == a)
}

pub fn token_by_symbol(s: &str) -> Option<Token> {
    tokens()
        .into_iter()
        .find(|t| t.symbol.eq_ignore_ascii_case(s))
}

/// A protocol id as people say it.
pub fn protocol_label(id: &str) -> &str {
    match id {
        "uniswap_v2" => "Uniswap V2",
        "uniswap_v3" => "Uniswap V3",
        "uniswap_v4" => "Uniswap V4",
        "uniswap_router" => "Uniswap routers",
        "pancake_v3" => "PancakeSwap V3",
        "balancer" => "Balancer",
        "aave_v3" => "Aave V3",
        "morpho_blue" => "Morpho Blue",
        "compound_v3" => "Compound V3",
        "fluid_vaults" => "Fluid",
        "unidentified" => "unidentified bots and routers",
        other => other,
    }
}

/// A wrapper as people say it.
pub fn wrapper_label(w: &str) -> &str {
    match w {
        "multicall3" => "a Multicall3 call",
        "self_multicall" => "a router multicall",
        "account_execute" => "a smart-account call",
        "entrypoint_v6" => "an ERC-4337 bundle (v0.6)",
        "entrypoint_v7" => "an ERC-4337 bundle (v0.7)",
        other => other,
    }
}

/// Functions worth naming. Not exhaustive: an unlisted selector is shown as hex.
pub const SELECTORS: [(&str, &str); 19] = [
    ("617ba037", "Aave: supply"),
    ("a415bcad", "Aave: borrow"),
    ("573ade81", "Aave: repay"),
    ("69328dec", "Aave: withdraw"),
    ("00a718a9", "Aave: liquidationCall"),
    ("a9059cbb", "token transfer"),
    ("095ea7b3", "token approve"),
    ("23b872dd", "token transferFrom"),
    ("04e45aaf", "Uniswap: exactInputSingle"),
    ("b858183f", "Uniswap: exactInput"),
    ("5ae401dc", "router multicall (with deadline)"),
    ("ac9650d8", "router multicall"),
    ("3593564c", "Universal Router: execute"),
    ("24856bc3", "Universal Router: execute (no deadline)"),
    ("82ad56cb", "Multicall3: aggregate3"),
    ("88316456", "Uniswap V3: mint position"),
    ("219f5d17", "Uniswap V3: increase liquidity"),
    ("0c49ccbe", "Uniswap V3: decrease liquidity"),
    ("fc6f7865", "Uniswap V3: collect fees"),
];

pub fn selector_name(hex8: &str) -> Option<&'static str> {
    let h = hex8.trim_start_matches("0x").to_ascii_lowercase();
    SELECTORS.iter().find(|(s, _)| *s == h).map(|(_, n)| *n)
}

/// `0x1234…abcd`.
pub fn short(a: &str) -> String {
    if a.len() > 14 {
        format!("{}…{}", &a[..8], &a[a.len() - 4..])
    } else {
        a.to_string()
    }
}

/// A contract name without its parenthesised detail: "Uniswap SwapRouter02 (exact-in swaps…)" -> "Uniswap SwapRouter02".
pub fn plain_contract_name(n: &str) -> &str {
    n.split(" (").next().unwrap_or(n)
}

/// What to call an address: a token symbol, an identified contract, or its short hex.
pub fn address_name(a: Address) -> String {
    if let Some(t) = token_by_address(a) {
        return t.symbol.to_string();
    }
    if let Some(c) = mev_decode::known_contract(a) {
        return plain_contract_name(c.name).to_string();
    }
    short(&format!("{a:#x}"))
}

/// A raw token amount as a human one: `1234567890` with 6 decimals is `1,234.57`.
pub fn human_amount(raw: &str, decimals: u8) -> String {
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return raw.to_string();
    }
    let d = decimals as usize;
    let padded = if digits.len() <= d {
        format!("{}{}", "0".repeat(d + 1 - digits.len()), digits)
    } else {
        digits
    };
    let (int, frac) = padded.split_at(padded.len() - d);
    let int = group_thousands(int.trim_start_matches('0').max("0"));
    let frac = frac.trim_end_matches('0');
    // Two decimals for ordinary amounts, more for tiny ones so they do not read as zero.
    let keep = if int == "0" {
        frac.len().min(6)
    } else {
        frac.len().min(2)
    };
    if keep == 0 || frac.is_empty() {
        int
    } else {
        format!("{int}.{}", &frac[..keep])
    }
}

pub fn group_thousands(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in b.iter().enumerate() {
        if i > 0 && (b.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*c as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_read_like_money() {
        assert_eq!(human_amount("1234567890", 6), "1,234.56");
        assert_eq!(human_amount("1000000", 6), "1");
        assert_eq!(human_amount("500000", 6), "0.5");
        assert_eq!(human_amount("1", 6), "0.000001");
        assert_eq!(human_amount("0", 6), "0");
        assert_eq!(human_amount("1500000000000000000", 18), "1.5");
        assert_eq!(human_amount("123456789012345678901234", 18), "123,456.78");
        assert_eq!(human_amount("42", 0), "42");
        assert_eq!(human_amount("not a number", 6), "not a number");
    }

    #[test]
    fn the_token_table_is_well_formed() {
        let ts = tokens();
        assert_eq!(ts.len(), TOKENS.len());
        let mut seen = std::collections::HashSet::new();
        for t in &ts {
            assert!(seen.insert(t.address), "duplicate {}", t.symbol);
        }
        assert_eq!(token_by_symbol("usdc").unwrap().decimals, 6);
        assert_eq!(
            token_by_address(token_by_symbol("WETH").unwrap().address)
                .unwrap()
                .symbol,
            "WETH"
        );
        assert!(token_by_symbol("NOPE").is_none());
    }

    #[test]
    fn names_fall_back_to_short_hex() {
        assert_eq!(
            address_name(token_by_symbol("USDC").unwrap().address),
            "USDC"
        );
        assert_eq!(address_name(Address::with_last_byte(1)), "0x000000…0001");
        assert_eq!(
            plain_contract_name("Uniswap SwapRouter02 (exact-in swaps and multicall decoded)"),
            "Uniswap SwapRouter02"
        );
        assert_eq!(selector_name("0xa415bcad"), Some("Aave: borrow"));
        assert_eq!(selector_name("deadbeef"), None);
        assert_eq!(protocol_label("aave_v3"), "Aave V3");
        assert_eq!(protocol_label("something_new"), "something_new");
        assert_eq!(group_thousands("1234567"), "1,234,567");
        assert_eq!(group_thousands("12"), "12");
    }
}
