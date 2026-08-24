//! The two shaping rules every `reshape` section prints through.
//!
//! Here rather than beside either caller because both need them.
//! [`super::reshape`] lays out the sections and [`super::recipes`] writes
//! the instruction inside them; while these lived in `reshape`, `recipes`
//! reached back for them and the two files sat in a loop — which the
//! cycles recipe caught, ranked (one entity out, nine back) and prescribed
//! this exact move for. A definition both ends need belongs under both,
//! not inside one of them.

/// How many offenders of one kind get named before a list summarises.
/// A folder with sixty breaches does not need sixty lines to make the
/// point, and the agent has the endpoint if it wants them all.
pub(super) const MAX_LISTED: usize = 20;

/// A sub-score, or an em dash for one that was not measured. Never `0.00`
/// for absent: zero is the failing end of every scale here, so printing it
/// for "no data" would report the worst possible news about nothing.
pub(super) fn num(value: Option<f32>) -> String {
    value.map_or_else(|| "—".to_string(), |v| format!("{v:.2}"))
}

/// A bullet list, truncated with a count rather than silently cut. A tool
/// that lists 20 of 60 offenders and does not say so reads as a complete
/// answer.
pub(super) fn listed(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let items: Vec<String> = items.into_iter().collect();
    let total = items.len();
    let mut out: Vec<String> = items
        .into_iter()
        .take(MAX_LISTED)
        .map(|i| format!("- {i}"))
        .collect();
    if total > MAX_LISTED {
        out.push(format!("- … and {} more.", total - MAX_LISTED));
    }
    out
}
