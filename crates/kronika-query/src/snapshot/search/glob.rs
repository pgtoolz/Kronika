//! Bounded Unicode-insensitive glob matching.

/// Case-insensitive bounded glob used by snapshot search values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobPattern(Vec<GlobToken>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GlobToken {
    Star,
    Any,
    Literal(char),
}

impl GlobPattern {
    /// An anchored, literal-only pattern: matches a candidate equal to
    /// `raw` case-insensitively, never a substring. `*` and `?` are
    /// literal characters here, not wildcards.
    #[must_use]
    pub fn exact(raw: &str) -> Self {
        Self(raw.chars().map(GlobToken::Literal).collect())
    }

    #[must_use]
    /// Build a case-insensitive substring pattern with literal wildcard characters.
    pub fn contains(raw: &str) -> Self {
        let mut tokens = Vec::with_capacity(raw.chars().count() + 2);
        tokens.push(GlobToken::Star);
        tokens.extend(raw.chars().map(GlobToken::Literal));
        tokens.push(GlobToken::Star);
        Self(tokens)
    }

    #[must_use]
    /// Build the text DSL's case-insensitive substring glob.
    pub fn new(raw: &str) -> Self {
        let mut tokens = vec![GlobToken::Star];
        for character in raw.chars() {
            let token = match character {
                '*' => GlobToken::Star,
                '?' => GlobToken::Any,
                literal => GlobToken::Literal(literal),
            };
            if token != GlobToken::Star || tokens.last() != Some(&GlobToken::Star) {
                tokens.push(token);
            }
        }
        if tokens.last() != Some(&GlobToken::Star) {
            tokens.push(GlobToken::Star);
        }
        Self(tokens)
    }

    #[must_use]
    /// Test a candidate against this pattern.
    pub fn matches(&self, candidate: &str) -> bool {
        let mut pattern_index = 0;
        let mut candidate_index = 0;
        let mut star = None;
        let mut retry = 0;
        while candidate_index < candidate.len() {
            let Some(character) = candidate
                .get(candidate_index..)
                .and_then(|remaining| remaining.chars().next())
            else {
                return false;
            };
            match self.0.get(pattern_index) {
                Some(GlobToken::Literal(wanted)) if unicode_char_equal(*wanted, character) => {
                    pattern_index += 1;
                    candidate_index += character.len_utf8();
                }
                Some(GlobToken::Any) => {
                    pattern_index += 1;
                    candidate_index += character.len_utf8();
                }
                Some(GlobToken::Star) => {
                    star = Some(pattern_index);
                    pattern_index += 1;
                    retry = candidate_index;
                }
                _ if let Some(star_index) = star => {
                    let Some(retry_character) = candidate
                        .get(retry..)
                        .and_then(|remaining| remaining.chars().next())
                    else {
                        return false;
                    };
                    retry += retry_character.len_utf8();
                    candidate_index = retry;
                    pattern_index = star_index + 1;
                }
                _ => return false,
            }
        }
        while self.0.get(pattern_index) == Some(&GlobToken::Star) {
            pattern_index += 1;
        }
        pattern_index == self.0.len()
    }

    #[must_use]
    /// Compare two anchored literal patterns case-insensitively.
    pub fn same_exact(&self, other: &Self) -> bool {
        self.0.len() == other.0.len()
            && self
                .0
                .iter()
                .zip(&other.0)
                .all(|(left, right)| match (left, right) {
                    (GlobToken::Literal(left), GlobToken::Literal(right)) => {
                        unicode_char_equal(*left, *right)
                    }
                    _ => left == right,
                })
    }
}

fn unicode_char_equal(left: char, right: char) -> bool {
    left.to_lowercase().eq(right.to_lowercase())
}
