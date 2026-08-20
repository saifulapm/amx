//! Walking past the paint a terminal writes into a captured screen.
//!
//! tmux's `-e` capture keeps every SGR sequence the pane drew — an attribute
//! change wherever a real program set a color or a weight, sometimes in the
//! middle of a word. A reader wants the words, not the paint, and so does a
//! rule matching against them.
//!
//! `strip_ansi` walks a captured screen once, character by character, and
//! answers with what a person would have read off it.

/// Remove every escape sequence from a captured screen, keeping the rest.
///
/// A `CSI` sequence (`ESC [`) runs from its parameters to a final byte in
/// `@`..=`~`; an `OSC` sequence (`ESC ]`) runs to a `BEL` or to `ESC \\`; any
/// other escape is exactly two bytes, and both are paint.
pub fn strip_ansi(screen: &str) -> String {
    let mut out = String::with_capacity(screen.len());
    let mut chars = screen.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_with_no_escapes_survives() {
        assert_eq!(strip_ansi("hello world"), "hello world");
    }

    #[test]
    fn a_csi_sequence_is_removed() {
        assert_eq!(strip_ansi("\u{1b}[1mbold\u{1b}[0m"), "bold");
    }

    #[test]
    fn a_csi_sequence_with_several_parameters_is_removed() {
        assert_eq!(strip_ansi("\u{1b}[38;5;208morange\u{1b}[39m"), "orange");
    }

    #[test]
    fn an_osc_sequence_ended_by_bel_is_removed() {
        assert_eq!(strip_ansi("\u{1b}]0;title\u{7}text"), "text");
    }

    #[test]
    fn an_osc_sequence_ended_by_a_string_terminator_is_removed() {
        assert_eq!(strip_ansi("\u{1b}]0;title\u{1b}\\text"), "text");
    }

    #[test]
    fn any_other_escape_is_exactly_two_bytes() {
        assert_eq!(strip_ansi("a\u{1b}Mb"), "ab");
    }

    #[test]
    fn a_lone_escape_at_the_end_of_the_screen_does_not_panic() {
        assert_eq!(strip_ansi("word\u{1b}"), "word");
    }

    #[test]
    fn rows_that_are_only_paint_still_carry_their_newline() {
        assert_eq!(strip_ansi("\u{1b}[0m\n\u{1b}[0m\n"), "\n\n");
    }

    /// A tiny deterministic generator, so the property test below is
    /// reproducible without pulling in a crate for it.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        fn below(&mut self, bound: u64) -> u64 {
            self.next() % bound
        }
    }

    /// Plain text a real pane could hold: letters, spacing, a little
    /// punctuation, and enough non-ASCII to prove the walk counts characters,
    /// not bytes. Never `ESC` itself — that would make the text its own
    /// escape sequence, which is not what this property is about.
    fn plain_text(seed: u64) -> String {
        const ALPHABET: &[char] = &[
            'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', ' ',
            ' ', '\n', '.', ',', '!', '?', '-', '_', '0', '1', '2', '3', '4', '5', 'é', '中', '🙂',
        ];
        let mut rng = Rng(seed);
        let len = rng.below(40);
        (0..len)
            .map(|_| ALPHABET[rng.below(ALPHABET.len() as u64) as usize])
            .collect()
    }

    /// A complete, valid escape sequence: one of the three shapes `strip_ansi`
    /// knows how to walk past.
    fn paint_one(out: &mut String, rng: &mut Rng) {
        match rng.below(3) {
            0 => {
                out.push('\u{1b}');
                out.push('[');
                let params = rng.below(3);
                for i in 0..params {
                    if i > 0 {
                        out.push(';');
                    }
                    out.push_str(&rng.below(256).to_string());
                }
                const FINALS: &[char] = &['m', 'H', 'J', 'K', 'A', 'B'];
                out.push(FINALS[rng.below(FINALS.len() as u64) as usize]);
            }
            1 => {
                out.push_str("\u{1b}]0;title");
                if rng.below(2) == 0 {
                    out.push('\u{7}');
                } else {
                    out.push_str("\u{1b}\\");
                }
            }
            _ => {
                const OTHERS: &[char] = &['M', '7', '8', 'c', 'D'];
                out.push('\u{1b}');
                out.push(OTHERS[rng.below(OTHERS.len() as u64) as usize]);
            }
        }
    }

    /// `plain`, as a real terminal might have painted it: an attribute change
    /// dropped in front of some of its characters and, sometimes, after the
    /// last of them.
    fn painted(plain: &str, seed: u64) -> String {
        let mut rng = Rng(seed ^ 0xA5A5_A5A5_A5A5_A5A5);
        let mut out = String::new();
        for c in plain.chars() {
            while rng.below(3) == 0 {
                paint_one(&mut out, &mut rng);
            }
            out.push(c);
        }
        while rng.below(3) == 0 {
            paint_one(&mut out, &mut rng);
        }
        out
    }

    #[test]
    fn strip_undoes_paint_for_any_plain_text() {
        for seed in 0..1000u64 {
            let plain = plain_text(seed);
            let screen = painted(&plain, seed);
            assert_eq!(strip_ansi(&screen), plain, "seed {seed}: {screen:?}");
        }
    }
}
