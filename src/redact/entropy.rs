//! Entropy calculation for secret detection

use std::collections::HashMap;

pub fn calculate_entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }

    let mut counts: HashMap<char, usize> = HashMap::new();
    for ch in s.chars() {
        *counts.entry(ch).or_insert(0) += 1;
    }

    let len = s.chars().count() as f64;
    counts
        .values()
        .map(|count| {
            let p = *count as f64 / len;
            -(p * p.log2())
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::calculate_entropy;

    #[test]
    fn entropy_is_zero_for_repeated_chars() {
        assert_eq!(calculate_entropy("aaaaaa"), 0.0);
    }

    #[test]
    fn entropy_higher_for_mixed_string() {
        assert!(calculate_entropy("a1b2c3d4") > calculate_entropy("aaaaaaaa"));
    }
}
