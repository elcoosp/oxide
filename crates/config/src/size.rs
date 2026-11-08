//! Size literal parsing (§14: `cache.l0.maxSize`, `budgets.clientJs`, …).
//!
//! Binary units: `b` = 1, `kb` = 1024, `mb` = 1024², `gb` = 1024³, `tb` = 1024⁴,
//! case-insensitive, with an optional decimal fraction (`1.5mb`). Plain digit
//! strings are bytes. Anything else → `CFG-SIZE`.

use crate::{Result, err};

/// The unit multipliers, as `f64` literals (the multiply happens in f64; a
/// `u64 as f64` conversion would lose precision for `tb` — the literal is
/// exact).
fn multiplier(unit: &str) -> Option<f64> {
    match unit {
        "b" => Some(1.0),
        "k" | "kb" => Some(1_024.0),
        "m" | "mb" => Some(1_048_576.0),
        "g" | "gb" => Some(1_073_741_824.0),
        "t" | "tb" => Some(1_099_511_627_776.0),
        _ => None,
    }
}

pub(crate) fn parse(s: &str) -> Result<u64> {
    let t = s.trim().to_ascii_lowercase();
    let t: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    if t.is_empty() {
        return Err(err("CFG-SIZE", format!("invalid size `{s}`: empty string")));
    }
    if let Ok(n) = t.parse::<u64>() {
        return Ok(n);
    }
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'));
    let (num, unit) = match split {
        Some(i) => t.split_at(i),
        None => (t.as_str(), ""),
    };
    let Some(mult) = multiplier(unit) else {
        return Err(err(
            "CFG-SIZE",
            format!("invalid size `{s}`: unknown unit `{unit}` (expected b|kb|mb|gb|tb)"),
        ));
    };
    let n: f64 = num.parse::<f64>().map_err(|_| {
        err(
            "CFG-SIZE",
            format!("invalid size `{s}`: bad number `{num}`"),
        )
    })?;
    if !n.is_finite() || n < 0.0 {
        return Err(err(
            "CFG-SIZE",
            format!("invalid size `{s}`: must be a non-negative finite number"),
        ));
    }
    // The old `as u64` cast truncated toward zero (floor on this
    // non-negative domain) and saturated above `u64::MAX` (float→int `as`
    // semantics, Rust 1.45+). Reproduce that without the casts the pedantic
    // lints flag: after `floor` the value is integral, so the decimal
    // round-trip is exact; anything beyond u64::MAX (or NaN, unreachable
    // here) saturates.
    let floored = (n * mult).floor();
    Ok(format!("{floored:.0}").parse::<u64>().unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_binary_units_and_fractions() {
        assert_eq!(parse("64kb").unwrap(), 65_536);
        assert_eq!(parse("1.5mb").unwrap(), 1_572_864);
        assert_eq!(parse("2gb").unwrap(), 2_147_483_648);
        assert_eq!(parse("7").unwrap(), 7);
    }

    #[test]
    fn rejects_garbage_with_cfg_size() {
        for bad in ["abc", "-5", "12zb", "", "1.2.3mb"] {
            assert_eq!(parse(bad).unwrap_err().0.code, "CFG-SIZE", "`{bad}`");
        }
    }
}
