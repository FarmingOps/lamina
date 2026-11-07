//! Small text helpers that several operations use.

pub use lamina_parts::provider::today;

/// Compare pad numbers so that 2 < 10 and A2 < A10.
pub fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    fn split(s: &str) -> (String, u64) {
        let p = s.trim_end_matches(|c: char| c.is_ascii_digit());
        (p.to_string(), s[p.len()..].parse().unwrap_or(0))
    }
    let (pa, na) = split(a);
    let (pb, nb) = split(b);
    pa.cmp(&pb).then(na.cmp(&nb)).then(a.cmp(b))
}
