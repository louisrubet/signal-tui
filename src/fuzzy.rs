//! Fuzzy search of emoji by shortcode, ranked like fzf: the typed letters in order,
//! preferably together and at the start of words.

/// An emoji found for a query, with the byte positions of the matched letters in its
/// shortcode.
#[derive(Debug, Clone, PartialEq)]
pub struct EmojiMatch {
    pub shortcode: &'static str,
    pub emoji: &'static str,
    pub positions: Vec<usize>,
}

// fzf's scores (its `algo.go`).
const MATCH: i32 = 16;
const GAP_START: i32 = 3;
const GAP_EXTENSION: i32 = 1;
/// At the start of the shortcode.
const START: i32 = 10;
/// At the start of a word in it (after `_` or `-`).
const BOUNDARY: i32 = 8;
/// After the previous letter, at least.
const CONSECUTIVE: i32 = 4;
/// The bonus of the first letter typed counts double.
const FIRST_MULTIPLIER: i32 = 2;

/// Score of `candidate` for `query` (both ASCII, lowercase) with the positions of the
/// matched letters, or `None` when the letters are not all there in order.
pub fn score(query: &str, candidate: &str) -> Option<(i32, Vec<usize>)> {
    let (q, c) = (query.as_bytes(), candidate.as_bytes());
    if q.is_empty() {
        return Some((0, Vec::new()));
    }
    // The first place where all the letters are found, then the latest start before it:
    // the shortest window.
    let mut end = 0;
    let mut next = 0;
    for (i, &b) in c.iter().enumerate() {
        if b == q[next] {
            next += 1;
            if next == q.len() {
                end = i;
                break;
            }
        }
    }
    if next < q.len() {
        return None;
    }
    let mut positions = Vec::with_capacity(q.len());
    let mut wanted = q.len();
    for i in (0..=end).rev() {
        if c[i] == q[wanted - 1] {
            positions.push(i);
            wanted -= 1;
            if wanted == 0 {
                break;
            }
        }
    }
    positions.reverse();

    let mut total = 0;
    // Bonus of the first letter of the current run of consecutive matches.
    let mut run_bonus = 0;
    for (n, &p) in positions.iter().enumerate() {
        let mut bonus = if p == 0 {
            START
        } else if matches!(c[p - 1], b'_' | b'-') {
            BOUNDARY
        } else {
            0
        };
        if n > 0 && p == positions[n - 1] + 1 {
            bonus = bonus.max(run_bonus).max(CONSECUTIVE);
        } else {
            run_bonus = bonus;
            if n > 0 {
                let gap = (p - positions[n - 1] - 1) as i32;
                total -= GAP_START + (gap - 1) * GAP_EXTENSION;
            }
        }
        if n == 0 {
            bonus *= FIRST_MULTIPLIER;
        }
        total += MATCH + bonus;
    }
    Some((total, positions))
}

/// The emoji whose shortcode matches `query`, best first, each once (under its best
/// shortcode). With an empty query, all of them, by shortcode.
pub fn emoji_matches(query: &str) -> Vec<EmojiMatch> {
    let query = query.to_ascii_lowercase();
    let mut scored: Vec<(i32, EmojiMatch)> = emoji_expander::EMOJIS
        .iter()
        .filter_map(|(shortcode, emoji)| {
            score(&query, shortcode).map(|(score, positions)| {
                (
                    score,
                    EmojiMatch {
                        shortcode,
                        emoji,
                        positions,
                    },
                )
            })
        })
        .collect();
    // Shorter shortcodes first on a tie: the exact one before its longer variants.
    scored.sort_by(|(sa, a), (sb, b)| {
        sb.cmp(sa)
            .then(a.shortcode.len().cmp(&b.shortcode.len()))
            .then(a.shortcode.cmp(b.shortcode))
    });
    if query.is_empty() {
        scored.sort_by(|(_, a), (_, b)| a.shortcode.cmp(b.shortcode));
    }
    let mut seen = std::collections::HashSet::new();
    scored
        .into_iter()
        .map(|(_, m)| m)
        .filter(|m| seen.insert(m.emoji))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_must_come_in_order() {
        assert_eq!(score("rkt", "rocket").map(|(_, p)| p), Some(vec![0, 3, 5]));
        assert_eq!(score("tkr", "rocket"), None);
    }

    #[test]
    fn together_and_at_word_starts_rank_higher() {
        let score = |q, c| score(q, c).unwrap().0;
        assert!(score("smile", "smile") > score("smile", "sweat_smile"));
        assert!(score("ok", "ok_hand") > score("ok", "smoking"));
        assert!(score("hp", "hot_pepper") > score("hp", "hippo"));
    }

    #[test]
    fn exact_shortcode_first() {
        let matches = emoji_matches("joy");
        assert_eq!(matches[0].emoji, "\u{1f602}");
        assert!(emoji_matches("thup").iter().any(|m| m.emoji == "\u{1f44d}"));
    }

    #[test]
    fn each_emoji_once() {
        let matches = emoji_matches("");
        let mut emojis: Vec<_> = matches.iter().map(|m| m.emoji).collect();
        let count = emojis.len();
        emojis.sort();
        emojis.dedup();
        assert_eq!(emojis.len(), count);
        assert!(count > 1000);
    }
}
