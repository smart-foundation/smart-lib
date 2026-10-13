/// Fuzzy search with typo tolerance and relevance scoring.

#[derive(Debug, Clone)]
pub struct FuzzyMatch {
    pub candidate: String,
    pub score: i64,
    pub indices: Vec<usize>,
}

/// Score a single character match.
fn char_score(query_char: char, candidate_char: char, pos: usize, prev_matched: bool) -> i64 {
    let mut score = 0i64;
    if query_char == candidate_char {
        score += 10;
    } else if query_char.to_ascii_lowercase() == candidate_char.to_ascii_lowercase() {
        score += 8;
    } else {
        return 0;
    }
    if prev_matched {
        score += 5;
    }
    if pos == 0 {
        score += 20;
    }
    if candidate_char == '_' || candidate_char == '-' || candidate_char == ' '
        || (pos > 0 && candidate_char.is_uppercase())
    {
        score += 15;
    }
    score
}

/// Check if candidate is a subsequence of query (characters in order).
/// Returns (is_match, score, matched_indices).
fn subsequence_match(query: &str, candidate: &str) -> (bool, i64, Vec<usize>) {
    let query_chars: Vec<char> = query.chars().collect();
    let candidate_chars: Vec<char> = candidate.chars().collect();

    if query_chars.is_empty() {
        return (true, 0, vec![]);
    }

    let mut qi = 0;
    let mut indices = Vec::new();
    let mut total_score = 0i64;
    let mut prev_matched = false;

    for (ci, &cc) in candidate_chars.iter().enumerate() {
        if qi < query_chars.len() && query_chars[qi].to_ascii_lowercase() == cc.to_ascii_lowercase() {
            total_score += char_score(query_chars[qi], cc, ci, prev_matched);
            indices.push(ci);
            prev_matched = true;
            qi += 1;
        } else {
            prev_matched = false;
        }
    }

    let matched = qi == query_chars.len();
    if matched {
        let coverage_bonus = (candidate.len() as i64 - query.len() as i64).max(0);
        total_score += 50 - coverage_bonus;
    }
    (matched, total_score, indices)
}

/// Compute Levenshtein edit distance between two strings.
fn edit_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    let mut dp = vec![vec![0usize; b_len + 1]; a_len + 1];
    for i in 0..=a_len {
        dp[i][0] = i;
    }
    for j in 0..=b_len {
        dp[0][j] = j;
    }
    for i in 1..=a_len {
        for j in 1..=b_len {
            let cost = if a_chars[i - 1] == b_chars[j - 1] { 0 } else { 1 };
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
        }
    }
    dp[a_len][b_len]
}

/// Perform fuzzy search with typo tolerance.
/// Returns matches sorted by score (best first).
pub fn fuzzy_search(query: &str, candidates: &[String]) -> Vec<FuzzyMatch> {
    let query_lower = query.to_lowercase();
    let mut results: Vec<FuzzyMatch> = Vec::new();

    for candidate in candidates {
        let (is_match, score, indices) = subsequence_match(&query_lower, &candidate.to_lowercase());

        if is_match {
            results.push(FuzzyMatch {
                candidate: candidate.clone(),
                score,
                indices,
            });
            continue;
        }

        let dist = edit_distance(&query_lower, &candidate.to_lowercase());
        let max_len = query_lower.len().max(candidate.len());
        let threshold = if max_len <= 3 { 1 } else { 2 };
        if dist <= threshold {
            let edit_score = 30 - (dist as i64 * 10);
            results.push(FuzzyMatch {
                candidate: candidate.clone(),
                score: edit_score,
                indices: vec![],
            });
        }
    }

    results.sort_by(|a, b| b.score.cmp(&a.score));
    results
}

/// Simple fuzzy match check: does the candidate match the query?
pub fn fuzzy_matches(query: &str, candidate: &str) -> bool {
    let (is_match, _, _) = subsequence_match(&query.to_lowercase(), &candidate.to_lowercase());
    if is_match {
        return true;
    }
    let dist = edit_distance(&query.to_lowercase(), &candidate.to_lowercase());
    let max_len = query.len().max(candidate.len());
    let threshold = if max_len <= 3 { 1 } else { 2 };
    dist <= threshold
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        let results = fuzzy_search("firefox", &["firefox".to_string()]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].candidate, "firefox");
    }

    #[test]
    fn test_subsequence_match() {
        let results = fuzzy_search("fir", &["firefox".to_string()]);
        assert!(!results.is_empty());
    }

    #[test]
    fn test_typo_tolerance() {
        let results = fuzzy_search("fiefox", &["firefox".to_string()]);
        assert!(!results.is_empty());
    }

    #[test]
    fn test_no_match() {
        let results = fuzzy_search("xyz", &["firefox".to_string()]);
        assert!(results.is_empty());
    }
}
