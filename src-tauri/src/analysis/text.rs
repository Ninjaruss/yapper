//! Word/filler primitives. Filler set is deliberately small and high-precision:
//! false filler-counts poison the baseline AND the live signal.

const FILLERS: [&str; 6] = ["um", "uh", "like", "you know", "kind of", "actually"];

/// Lowercase words, punctuation stripped from edges (apostrophes kept).
pub fn normalize_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

pub fn word_count(text: &str) -> usize {
    normalize_words(text).len()
}

/// Count filler occurrences: the multi-word fillers first (consumed greedily),
/// single-word fillers per token, plus utterance-leading "so".
pub fn count_fillers(text: &str) -> usize {
    let words = normalize_words(text);
    let mut count = 0;
    let mut i = 0;
    while i < words.len() {
        let two = if i + 1 < words.len() {
            format!("{} {}", words[i], words[i + 1])
        } else {
            String::new()
        };
        if FILLERS.contains(&two.as_str()) {
            count += 1;
            i += 2;
            continue;
        }
        if FILLERS.contains(&words[i].as_str()) || (i == 0 && words[i] == "so") {
            count += 1;
        }
        i += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_fillers_case_insensitively_and_multiword() {
        let t = "Um, so I was like, you know, actually thinking. Uh huh.";
        assert_eq!(count_fillers(t), 5); // um, like, you know, actually, uh — "so" at index 1 is not leading
    }

    #[test]
    fn leading_so_counts_but_medial_so_does_not() {
        assert_eq!(count_fillers("So I went home"), 1);
        assert_eq!(count_fillers("I did it so that it works"), 0);
    }

    #[test]
    fn word_count_ignores_punctuation_tokens() {
        assert_eq!(word_count("Hello, world — again!"), 3);
    }

    #[test]
    fn normalize_strips_punct_and_lowercases() {
        assert_eq!(normalize_words("It's DONE, right?"), vec!["it's", "done", "right"]);
    }
}
