//! Token estimation and budget packing.
//!
//! The estimate is deliberately crude — roughly four characters per token is
//! close enough for source code, and being 15% out never changes a decision
//! this makes. Shipping a real tokenizer would mean a large dependency and a
//! model-specific answer, for no gain.

/// Rough token count for a string.
pub fn est_tokens(s: &str) -> usize {
    (s.len() / 4).max(1)
}

/// Tracks spend against a ceiling.
pub struct Budget {
    limit: usize,
    used: usize,
}

impl Budget {
    pub fn new(limit: usize) -> Self {
        Budget { limit, used: 0 }
    }

    /// Try to spend `n` tokens. Returns false if it would exceed the ceiling,
    /// leaving the budget untouched.
    pub fn try_spend(&mut self, n: usize) -> bool {
        if self.used + n > self.limit {
            return false;
        }
        self.used += n;
        true
    }

    pub fn used(&self) -> usize {
        self.used
    }

    pub fn limit(&self) -> usize {
        self.limit
    }
}

/// Trim a line to a sane width, marking that it was cut.
///
/// Minified bundles and generated files produce single lines megabytes long.
/// One of those would eat an entire budget and tell the caller nothing.
pub fn clip(line: &str, max_chars: usize) -> String {
    let t = line.trim_end();
    if t.chars().count() <= max_chars {
        return t.to_string();
    }
    let cut: String = t.chars().take(max_chars).collect();
    format!("{cut} …[line clipped]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn est_is_never_zero() {
        assert_eq!(est_tokens(""), 1);
        assert!(est_tokens("a".repeat(400).as_str()) >= 100);
    }

    #[test]
    fn budget_refuses_overspend_without_consuming() {
        let mut b = Budget::new(10);
        assert!(b.try_spend(6));
        assert!(!b.try_spend(6));
        assert_eq!(b.used(), 6, "a refused spend must not consume budget");
        assert!(b.try_spend(4));
        assert_eq!(b.used(), b.limit());
    }

    #[test]
    fn clip_marks_truncation() {
        let long = "x".repeat(500);
        let out = clip(&long, 100);
        assert!(out.contains("[line clipped]"));
        assert!(out.len() < 200);
    }

    #[test]
    fn clip_leaves_short_lines_alone() {
        assert_eq!(clip("fn main() {}  ", 100), "fn main() {}");
    }
}
