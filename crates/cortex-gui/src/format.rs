//! Turning numbers into short text.

/// `950`, `21.6k`, `1.2M`.
pub fn count(value: u64) -> String {
    match value {
        0..1_000 => value.to_string(),
        1_000..1_000_000 => format!("{:.1}k", value as f64 / 1e3),
        _ => format!("{:.1}M", value as f64 / 1e6),
    }
}

/// `840 ms`, `2.1 s`, `3 min 5 s`.
pub fn duration(milliseconds: u64) -> String {
    match milliseconds {
        0..1_000 => format!("{milliseconds} ms"),
        1_000..60_000 => format!("{:.1} s", milliseconds as f64 / 1e3),
        _ => format!(
            "{} min {} s",
            milliseconds / 60_000,
            milliseconds % 60_000 / 1_000
        ),
    }
}

pub fn percent(ratio: f32) -> String {
    format!("{:.0}%", ratio * 100.0)
}

/// The text on one line, cut to `max_chars` with an ellipsis.
pub fn one_line(text: &str, max_chars: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");

    if flat.chars().count() <= max_chars {
        return flat;
    }

    let clipped: String = flat.chars().take(max_chars.saturating_sub(1)).collect();

    format!("{clipped}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_shortened() {
        assert_eq!(count(950), "950");
        assert_eq!(count(21_640), "21.6k");
        assert_eq!(count(1_200_000), "1.2M");
    }

    #[test]
    fn durations_pick_a_sensible_unit() {
        assert_eq!(duration(840), "840 ms");
        assert_eq!(duration(2_140), "2.1 s");
        assert_eq!(duration(185_000), "3 min 5 s");
    }

    #[test]
    fn long_text_is_cut_to_one_line() {
        assert_eq!(one_line("a\n  b", 10), "a b");
        assert_eq!(one_line("abcdefgh", 5), "abcd…");
    }
}
