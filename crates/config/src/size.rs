//! Size literal parsing (§14: `cache.l0MaxSize`, `budgets.clientJs`, …).
//!
//! Binary units: `b` = 1, `kb` = 1024, `mb` = 1024², `gb` = 1024³, `tb` = 1024⁴,
//! case-insensitive, with an optional decimal fraction (`1.5mb`). Plain digit
//! strings are bytes. Anything else → `CFG-SIZE`.

use crate::{Result, err};

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
    let mult: u64 = match unit {
        "b" => 1,
        "k" | "kb" => 1024,
        "m" | "mb" => 1024 * 1024,
        "g" | "gb" => 1024 * 1024 * 1024,
        "t" | "tb" => 1024u64 * 1024 * 1024 * 1024,
        _ => {
            return Err(err(
                "CFG-SIZE",
                format!("invalid size `{s}`: unknown unit `{unit}` (expected b|kb|mb|gb|tb)"),
            ));
        }
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
    Ok((n * mult as f64) as u64)
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
