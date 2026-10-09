//! Byte display and size-input parsing (FR05).
//!
//! Bytes are integers everywhere. Display uses decimal units (1 GB is
//! 1,000,000,000 bytes). Input accepts decimal units and explicit binary
//! units (1 GiB is 1,073,741,824 bytes). Parsing is exact: it rejects
//! negative, malformed, fractional-byte and overflowing input instead of
//! truncating.

use std::fmt;

const DECIMAL_UNITS: [(&str, u128); 6] = [
    ("PB", 1_000_000_000_000_000),
    ("TB", 1_000_000_000_000),
    ("GB", 1_000_000_000),
    ("MB", 1_000_000),
    ("KB", 1_000),
    ("B", 1),
];

/// Short decimal display, e.g. `0 B`, `999 B`, `1.5 KB`, `88 GB`.
///
/// Values below 10 of a unit get one decimal place; larger values are whole
/// numbers. Rounding is half-up, and a value that rounds up to 1000 of a
/// unit moves to the next unit.
pub fn format_decimal(bytes: u128) -> String {
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut i = DECIMAL_UNITS
        .iter()
        .position(|&(_, u)| bytes >= u)
        .unwrap_or(5);
    loop {
        let (unit_name, unit) = DECIMAL_UNITS[i];
        let tenths = (bytes * 10 + unit / 2) / unit;
        if tenths < 100 {
            return format!("{}.{} {unit_name}", tenths / 10, tenths % 10);
        }
        let whole = (bytes + unit / 2) / unit;
        if whole < 1000 || i == 0 {
            return format!("{whole} {unit_name}");
        }
        i -= 1;
    }
}

/// Exact count with thousands separators, e.g. `1,234,567 bytes`.
pub fn format_exact(bytes: u128) -> String {
    let digits = bytes.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 6);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out.push_str(if bytes == 1 { " byte" } else { " bytes" });
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SizeParseError {
    Empty,
    Negative,
    InvalidNumber,
    UnknownUnit(String),
    /// The value is not a whole number of bytes, e.g. `0.5 B`.
    FractionalBytes,
    Overflow,
}

impl fmt::Display for SizeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SizeParseError::Empty => write!(f, "enter a size"),
            SizeParseError::Negative => write!(f, "size cannot be negative"),
            SizeParseError::InvalidNumber => write!(f, "not a valid number"),
            SizeParseError::UnknownUnit(u) => {
                write!(
                    f,
                    "unknown unit \"{u}\" (use B, KB, MB, GB, TB, KiB, MiB, GiB or TiB)"
                )
            }
            SizeParseError::FractionalBytes => write!(f, "size must be a whole number of bytes"),
            SizeParseError::Overflow => write!(f, "size is too large"),
        }
    }
}

impl std::error::Error for SizeParseError {}

fn unit_multiplier(unit: &str) -> Option<u128> {
    Some(match unit.to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "kb" => 1_000,
        "mb" => 1_000_000,
        "gb" => 1_000_000_000,
        "tb" => 1_000_000_000_000,
        "kib" => 1 << 10,
        "mib" => 1 << 20,
        "gib" => 1 << 30,
        "tib" => 1 << 40,
        _ => return None,
    })
}

/// Parse a size such as `500 MB`, `1.5GB`, `2 GiB` or `1024`.
pub fn parse_size(input: &str) -> Result<u64, SizeParseError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(SizeParseError::Empty);
    }
    if s.starts_with('-') {
        return Err(SizeParseError::Negative);
    }
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (number, unit) = (&s[..split], s[split..].trim());
    let multiplier =
        unit_multiplier(unit).ok_or_else(|| SizeParseError::UnknownUnit(unit.to_string()))?;

    let (int_part, frac_part) = match number.split_once('.') {
        Some((i, f)) => (i, f),
        None => (number, ""),
    };
    if int_part.is_empty() && frac_part.is_empty() {
        return Err(SizeParseError::InvalidNumber);
    }
    if frac_part.contains('.') {
        return Err(SizeParseError::InvalidNumber);
    }
    let parse_digits = |d: &str| -> Result<u128, SizeParseError> {
        if d.is_empty() {
            return Ok(0);
        }
        // More than 39 digits cannot fit in u128.
        if d.len() > 38 {
            return Err(SizeParseError::Overflow);
        }
        d.parse::<u128>().map_err(|_| SizeParseError::InvalidNumber)
    };
    let int_value = parse_digits(int_part)?;
    let frac_digits = frac_part.trim_end_matches('0');
    let frac_value = parse_digits(frac_digits)?;
    let scale = 10u128
        .checked_pow(frac_digits.len() as u32)
        .ok_or(SizeParseError::Overflow)?;

    let whole = int_value
        .checked_mul(multiplier)
        .ok_or(SizeParseError::Overflow)?;
    let frac_numerator = frac_value
        .checked_mul(multiplier)
        .ok_or(SizeParseError::Overflow)?;
    if frac_numerator % scale != 0 {
        return Err(SizeParseError::FractionalBytes);
    }
    let total = whole
        .checked_add(frac_numerator / scale)
        .ok_or(SizeParseError::Overflow)?;
    u64::try_from(total).map_err(|_| SizeParseError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_small_values_in_bytes() {
        assert_eq!(format_decimal(0), "0 B");
        assert_eq!(format_decimal(999), "999 B");
    }

    #[test]
    fn formats_with_one_decimal_below_ten() {
        assert_eq!(format_decimal(1_000), "1.0 KB");
        assert_eq!(format_decimal(1_500), "1.5 KB");
        assert_eq!(format_decimal(1_049), "1.0 KB");
        assert_eq!(format_decimal(1_050), "1.1 KB");
        assert_eq!(format_decimal(9_949), "9.9 KB");
    }

    #[test]
    fn formats_whole_numbers_from_ten() {
        assert_eq!(format_decimal(9_950), "10 KB");
        assert_eq!(format_decimal(88_000_000_000), "88 GB");
        assert_eq!(format_decimal(512_400_000), "512 MB");
    }

    #[test]
    fn rounding_up_to_1000_moves_to_next_unit() {
        assert_eq!(format_decimal(999_500), "1.0 MB");
        assert_eq!(format_decimal(999_499), "999 KB");
    }

    #[test]
    fn exact_format_has_separators() {
        assert_eq!(format_exact(0), "0 bytes");
        assert_eq!(format_exact(1), "1 byte");
        assert_eq!(format_exact(1_234_567), "1,234,567 bytes");
        assert_eq!(format_exact(100), "100 bytes");
    }

    #[test]
    fn parses_units() {
        assert_eq!(parse_size("1024"), Ok(1024));
        assert_eq!(parse_size("500 MB"), Ok(500_000_000));
        assert_eq!(parse_size("1.5GB"), Ok(1_500_000_000));
        assert_eq!(parse_size(" 2 gib "), Ok(2 * 1_073_741_824));
        assert_eq!(parse_size("0.5 KiB"), Ok(512));
        assert_eq!(parse_size("1.50 kb"), Ok(1_500));
        assert_eq!(parse_size(".5 KB"), Ok(500));
        assert_eq!(parse_size("3."), Ok(3));
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(parse_size(""), Err(SizeParseError::Empty));
        assert_eq!(parse_size("-5 MB"), Err(SizeParseError::Negative));
        assert_eq!(parse_size("."), Err(SizeParseError::InvalidNumber));
        assert_eq!(parse_size("1.2.3 MB"), Err(SizeParseError::InvalidNumber));
        assert_eq!(
            parse_size("5 XB"),
            Err(SizeParseError::UnknownUnit("XB".into()))
        );
        assert_eq!(
            parse_size("1e9"),
            Err(SizeParseError::UnknownUnit("e9".into()))
        );
        assert_eq!(parse_size("0.5 B"), Err(SizeParseError::FractionalBytes));
        assert_eq!(
            parse_size("1.0001 KB"),
            Err(SizeParseError::FractionalBytes)
        );
    }

    #[test]
    fn rejects_overflow() {
        assert_eq!(parse_size("18446744073709551615"), Ok(u64::MAX));
        assert_eq!(
            parse_size("18446744073709551616"),
            Err(SizeParseError::Overflow)
        );
        assert_eq!(parse_size("20000000 TB"), Err(SizeParseError::Overflow));
        assert_eq!(
            parse_size("99999999999999999999999999999999999999999999 GB"),
            Err(SizeParseError::Overflow)
        );
    }
}
