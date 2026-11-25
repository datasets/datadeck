use std::collections::HashMap;

pub const SAMPLE_LIMIT: usize = 1024;
const BINS: usize = 20;

#[derive(Default)]
pub(crate) struct Profile {
    pub rows: usize,
    pub missing: usize,
    pub distinct: usize,
    pub common: Vec<(String, usize)>,
    pub distribution: Option<Distribution>,
}

pub(crate) struct Distribution {
    pub min: String,
    pub max: String,
    pub bins: [usize; BINS],
    pub labels: [String; BINS],
}

pub(super) fn numeric(value: &str) -> Option<f64> {
    let value = value.trim();
    // Leading zeros in identifiers carry information.
    let unsigned = value.trim_start_matches(['-', '+']);
    if unsigned.starts_with('0') && unsigned.as_bytes().get(1).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    value.parse::<f64>().ok().filter(|n| n.is_finite())
}

impl Profile {
    pub fn read<'a>(rows: impl IntoIterator<Item = &'a Vec<String>>, column: usize) -> Self {
        let mut counts = HashMap::<&str, usize>::new();
        let mut numbers = Vec::new();
        let mut missing = 0;
        let mut total = 0;
        for row in rows {
            total += 1;
            let value = row.get(column).map(String::as_str).unwrap_or_default();
            if value.is_empty() {
                missing += 1;
                continue;
            }
            *counts.entry(value).or_default() += 1;
            if let Some(n) = numeric(value) {
                numbers.push((n, value));
            }
        }
        let distinct = counts.len();
        let mut ordered: Vec<_> = counts.into_iter().collect();
        ordered.sort_unstable_by(|(left, a), (right, b)| b.cmp(a).then(left.cmp(right)));
        let common = ordered
            .into_iter()
            .take(5)
            .map(|(s, n)| (s.chars().take(100).collect(), n))
            .collect();
        let distribution = if !numbers.is_empty() && numbers.len() == total - missing {
            numbers.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
            let (lo, lo_text) = numbers[0];
            let (hi, hi_text) = numbers[numbers.len() - 1];
            let mut bins = [0; BINS];
            let mut spans: [Option<(&str, &str)>; BINS] = [None; BINS];
            for (n, raw) in numbers {
                // Halving first avoids overflow for a range spanning both finite extremes.
                let fraction = if lo == hi {
                    0.5
                } else if (hi - lo).is_finite() {
                    (n - lo) / (hi - lo)
                } else {
                    (0.5 * n - 0.5 * lo) / (0.5 * hi - 0.5 * lo)
                };
                let slot = ((fraction * BINS as f64) as usize).min(BINS - 1);
                bins[slot] += 1;
                spans[slot].get_or_insert((raw, raw)).1 = raw;
            }
            Some(Distribution {
                min: lo_text.into(),
                max: hi_text.into(),
                bins,
                labels: std::array::from_fn(|i| match spans[i] {
                    None => "0 rows".into(),
                    Some((a, b)) => {
                        let range = if a == b {
                            a.to_owned()
                        } else {
                            format!("{a}–{b}")
                        };
                        format!(
                            "{range} · {} {}",
                            bins[i],
                            if bins[i] == 1 { "row" } else { "rows" }
                        )
                    }
                }),
            })
        } else {
            None
        };
        Self {
            rows: total,
            missing,
            distinct,
            common,
            distribution,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_for_missing_values_and_numeric_extremes() {
        let rows = vec![
            vec!["-1e308".into()],
            vec!["1e308".into()],
            vec!["0".into()],
            vec![],
        ];
        let p = Profile::read(&rows, 0);
        assert_eq!((p.rows, p.distinct, p.missing), (4, 3, 1));
        let histogram = p.distribution.unwrap();
        assert_eq!(histogram.bins.iter().sum::<usize>(), 3);
        assert_eq!((histogram.bins[0], histogram.bins[BINS - 1]), (1, 1));
    }

    #[test]
    fn treats_leading_zero_ids_as_text_and_orders_ties() {
        let rows = ["010", "007", "010", "002"].map(|s| vec![s.into()]);
        let p = Profile::read(&rows, 0);
        assert!(p.distribution.is_none());
        assert_eq!(
            p.common,
            [("010".into(), 2), ("002".into(), 1), ("007".into(), 1)]
        );
    }
}
