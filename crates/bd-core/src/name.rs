//! Native file names.
//!
//! Names are kept as `OsString`, which is lossless on every platform: on
//! Windows it can hold any UTF-16 sequence, including unpaired surrogates
//! that are not valid Unicode (FR03). Display strings are derived and must
//! never be used as action targets.

use std::cmp::Ordering;
use std::ffi::OsStr;

/// The unit of a native name used for ordinal comparison: UTF-16 code units
/// on Windows, bytes elsewhere.
#[cfg(windows)]
pub type OrdinalUnit = u16;
#[cfg(not(windows))]
pub type OrdinalUnit = u8;

/// Path separator in ordinal units.
#[cfg(windows)]
pub const ORDINAL_SEPARATOR: OrdinalUnit = b'\\' as u16;
#[cfg(not(windows))]
pub const ORDINAL_SEPARATOR: OrdinalUnit = b'/';

/// Append a name's ordinal units to `out`.
pub fn push_ordinal_units(name: &OsStr, out: &mut Vec<OrdinalUnit>) {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        out.extend(name.encode_wide());
    }
    #[cfg(not(windows))]
    {
        out.extend_from_slice(name.as_encoded_bytes());
    }
}

/// Case-sensitive ordinal comparison, matching Windows
/// `CompareStringOrdinal(..., bIgnoreCase = FALSE)` on Windows.
///
/// This is not the same as comparing UTF-8: UTF-16 code-unit order puts
/// surrogate pairs (U+10000 and above) before U+E000..U+FFFF.
pub fn ordinal_cmp(a: &OsStr, b: &OsStr) -> Ordering {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        a.encode_wide().cmp(b.encode_wide())
    }
    #[cfg(not(windows))]
    {
        a.as_encoded_bytes().cmp(b.as_encoded_bytes())
    }
}

/// Lower-cased, lossy form of a name used only for case-insensitive search
/// (FR05). Never used for identity.
pub fn fold_for_search(name: &OsStr) -> Box<str> {
    match name.to_str() {
        // Most names are ASCII: one allocation, no Unicode case tables.
        Some(s) if s.is_ascii() => s.to_ascii_lowercase().into_boxed_str(),
        _ => name.to_string_lossy().to_lowercase().into_boxed_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    use std::ffi::OsString;

    #[test]
    fn ordinal_is_case_sensitive() {
        assert_eq!(
            ordinal_cmp(OsStr::new("README.txt"), OsStr::new("Readme.txt")),
            Ordering::Less
        );
        assert_eq!(
            ordinal_cmp(OsStr::new("a"), OsStr::new("a")),
            Ordering::Equal
        );
    }

    #[test]
    fn fold_lowercases() {
        assert_eq!(
            &*fold_for_search(OsStr::new("Holiday VIDEO.MP4")),
            "holiday video.mp4"
        );
    }

    #[cfg(windows)]
    #[test]
    fn utf16_order_differs_from_utf8_order() {
        // U+FF5E (fullwidth tilde) is a single BMP code unit 0xFF5E.
        // U+1F600 (emoji) is the surrogate pair 0xD83D 0xDE00.
        // UTF-16 ordinal: emoji < tilde. UTF-8/code point: tilde < emoji.
        let tilde = OsString::from("\u{FF5E}");
        let emoji = OsString::from("\u{1F600}");
        assert_eq!(ordinal_cmp(&emoji, &tilde), Ordering::Less);
    }

    #[cfg(windows)]
    #[test]
    fn unpaired_surrogates_survive() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let units = [0x61u16, 0xD800, 0x62];
        let name = OsString::from_wide(&units);
        assert_eq!(name.encode_wide().collect::<Vec<_>>(), units);
        assert!(name.to_str().is_none());
    }

    #[test]
    fn ordinal_units_for_ascii() {
        let mut v = Vec::new();
        push_ordinal_units(OsStr::new("ab"), &mut v);
        v.push(ORDINAL_SEPARATOR);
        push_ordinal_units(OsStr::new("c"), &mut v);
        assert_eq!(v.len(), 4);
        assert_eq!(v[0], OrdinalUnit::from(b'a'));
        assert_eq!(v[2], ORDINAL_SEPARATOR);
        assert_eq!(v[3], OrdinalUnit::from(b'c'));
    }
}
