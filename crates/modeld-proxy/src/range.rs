//! HTTP `Range` header parsing for the blob endpoint.
//!
//! Parses a single-range `bytes=` request and returns `(start, end)` inclusive
//! byte offsets. Multi-range requests (`bytes=0-10,20-30`) return only the
//! first range (documented limitation — we never emit `multipart/byteranges`).

/// Parse a `Range: bytes=...` header value.
///
/// Supported forms (per RFC 7233):
/// - `bytes=0-1023`       → absolute range
/// - `bytes=500-`         → from byte 500 to end
/// - `bytes=-500`         → last 500 bytes
///
/// Returns `None` when the header is absent, malformed, or unsatisfiable.
/// `total_len` is the full content length (needed to resolve suffix ranges).
pub fn parse_range(header: Option<&str>, total_len: u64) -> Option<(u64, u64)> {
    let header = header?;
    let header = header.trim();
    let rest = header.strip_prefix("bytes=")?;
    // take only the first range spec (ignore multi-range)
    let spec = rest.split(',').next()?.trim();

    let (start, end) = if let Some((s, e)) = spec.split_once('-') {
        let s = s.trim();
        let e = e.trim();
        match (s.is_empty(), e.is_empty()) {
            // suffix range: bytes=-N → last N bytes
            (true, false) => {
                let n: u64 = e.parse().ok()?;
                if n == 0 {
                    return None;
                }
                let start = total_len.checked_sub(n).unwrap_or(0);
                (start, total_len.saturating_sub(1))
            }
            // open-ended: bytes=N-
            (false, true) => {
                let start: u64 = s.parse().ok()?;
                (start, total_len.saturating_sub(1))
            }
            // absolute: bytes=N-M
            (false, false) => {
                let start: u64 = s.parse().ok()?;
                let end: u64 = e.parse().ok()?;
                (start, end)
            }
            // "bytes=-" is invalid
            (true, true) => return None,
        }
    } else {
        return None;
    };

    // unsatisfiable / invalid
    if start > end || start >= total_len {
        return None;
    }
    // clamp end to last byte
    let end = end.min(total_len.saturating_sub(1));
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_header() {
        assert_eq!(parse_range(None, 1000), None);
    }

    #[test]
    fn test_absolute_range() {
        assert_eq!(parse_range(Some("bytes=0-1023"), 2000), Some((0, 1023)));
        assert_eq!(parse_range(Some("bytes=500-999"), 1000), Some((500, 999)));
    }

    #[test]
    fn test_open_ended_range() {
        assert_eq!(parse_range(Some("bytes=500-"), 1000), Some((500, 999)));
        assert_eq!(parse_range(Some("bytes=0-"), 1000), Some((0, 999)));
    }

    #[test]
    fn test_suffix_range() {
        assert_eq!(parse_range(Some("bytes=-500"), 1000), Some((500, 999)));
        assert_eq!(parse_range(Some("bytes=-1"), 1000), Some((999, 999)));
        // suffix larger than content → from 0
        assert_eq!(parse_range(Some("bytes=-2000"), 1000), Some((0, 999)));
    }

    #[test]
    fn test_multi_range_takes_first() {
        assert_eq!(parse_range(Some("bytes=0-10,20-30"), 1000), Some((0, 10)));
    }

    #[test]
    fn test_malformed() {
        assert_eq!(parse_range(Some("bytes=abc"), 1000), None);
        assert_eq!(parse_range(Some("bytes=-"), 1000), None);
        assert_eq!(parse_range(Some("items=0-10"), 1000), None);
        assert_eq!(parse_range(Some("bytes=-0"), 1000), None);
    }

    #[test]
    fn test_unsatisfiable() {
        assert_eq!(parse_range(Some("bytes=2000-3000"), 1000), None);
        // start == total_len is unsatisfiable
        assert_eq!(parse_range(Some("bytes=1000-"), 1000), None);
    }

    #[test]
    fn test_end_clamped_to_content() {
        // end beyond content → clamped to last byte
        assert_eq!(parse_range(Some("bytes=900-5000"), 1000), Some((900, 999)));
    }
}
