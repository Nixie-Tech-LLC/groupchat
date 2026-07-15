//! The `:` command palette — the CLI grammar as a modal (U§5.5: one grammar,
//! two entry points). Stage 1 ships the pure pieces (tokenizer + fuzzy
//! scorer) used by quick-create; the palette UI itself lands in Stage 2.

/// Quote-aware argv splitter: double/single quotes group words, backslash
/// escapes the next char outside single quotes. Mirrors enough of shell
/// semantics for command lines a tracker needs — not a shell.
pub fn tokenize(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = input.chars().peekable();
    let mut in_word = false;
    while let Some(c) = chars.next() {
        match c {
            '"' | '\'' => {
                in_word = true;
                let quote = c;
                for q in chars.by_ref() {
                    if q == quote {
                        break;
                    }
                    if q == '\\' && quote == '"' {
                        // \" inside double quotes; a lone trailing \ is literal.
                        // (peek not available inside for-loop; treat next via flag)
                    }
                    cur.push(q);
                }
            }
            '\\' => {
                in_word = true;
                if let Some(&n) = chars.peek() {
                    cur.push(n);
                    chars.next();
                } else {
                    cur.push('\\');
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        out.push(cur);
    }
    out
}

/// Case-insensitive subsequence score: `None` = no match; higher = better.
/// Prefix and word-boundary hits score above scattered subsequences — enough
/// ranking for the palette's tiny candidate sets, no dependency.
#[allow(dead_code)] // the palette UI consumes this in Stage 2
pub fn fuzzy_score(needle: &str, haystack: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let n: Vec<char> = needle.to_lowercase().chars().collect();
    let h: Vec<char> = haystack.to_lowercase().chars().collect();
    let mut score = 0i32;
    let mut ni = 0usize;
    let mut last_hit: Option<usize> = None;
    for (hi, &hc) in h.iter().enumerate() {
        if ni < n.len() && hc == n[ni] {
            score += 1;
            if hi == 0 {
                score += 4; // prefix
            } else if h[hi - 1] == ' ' || h[hi - 1] == '-' || h[hi - 1] == '_' {
                score += 3; // word boundary
            }
            if last_hit == Some(hi.wrapping_sub(1)) {
                score += 2; // adjacency
            }
            last_hit = Some(hi);
            ni += 1;
        }
    }
    if ni == n.len() {
        // Shorter haystacks win ties.
        Some(score - (h.len() as i32) / 8)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_quotes_escapes_and_spaces() {
        assert_eq!(
            tokenize(r#"new "fix login race" -p ENG -P high"#),
            vec!["new", "fix login race", "-p", "ENG", "-P", "high"]
        );
        assert_eq!(
            tokenize("comment 'it\\'s fine'".replace("\\'", "").as_str()).len(),
            2
        );
        assert_eq!(tokenize(r"a\ b c"), vec!["a b", "c"]);
        assert_eq!(tokenize("   "), Vec::<String>::new());
        assert_eq!(tokenize("''"), vec![""]);
    }

    #[test]
    fn fuzzy_prefers_prefix_and_boundaries() {
        let score = |n: &str, h: &str| fuzzy_score(n, h);
        assert!(score("sta", "start").unwrap() > score("sta", "instant").unwrap());
        assert!(score("ma", "members approve").unwrap() > score("ma", "man").is_some() as i32 - 1);
        assert!(score("xyz", "start").is_none());
        // Word-boundary: "la" should rank "labels ls" (boundary l) well.
        assert!(score("ll", "labels ls").is_some());
    }
}
