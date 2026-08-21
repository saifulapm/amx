//! Walking past the paint a terminal writes into a captured screen.
//!
//! tmux's `-e` capture keeps every SGR sequence the pane drew — an attribute
//! change wherever a real program set a color or a weight, sometimes in the
//! middle of a word. A reader wants the words, not the paint, and so does a
//! rule matching against them. The card wants both: what claude drew, drawn
//! the way claude drew it.
//!
//! **One walk, two collectors.** [`strip_ansi`] and [`painted`] are two ways of
//! reading the same private [`walk`], so neither re-implements the grammar and
//! the two cannot drift apart. The law between them is stated as a property
//! test at the bottom of this file: `strip_ansi(s)` is `painted(s)` with the
//! style thrown away and the rows joined by newlines.
//!
//! **This is not a display sanitiser.** It removes *escapes*;
//! [`crate::tmux::sanitize`] removes *characters a terminal must not be handed*
//! and stays where it is. Neither subsumes the other, and a capture taken with
//! its paint kept wants both, in that order: the escapes are walked into
//! styling first, and what falls out is made inert before anything draws it.

/// A colour in the three forms SGR carries one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Colour {
    /// SGR 30-37 and 90-97 for the foreground, 40-47 and 100-107 for the
    /// background, as the ANSI index 0-15.
    Ansi(u8),
    /// `38;5;n` and `48;5;n`.
    Indexed(u8),
    /// `38;2;r;g;b` and `48;2;r;g;b`.
    Rgb(u8, u8, u8),
}

/// One run of text and the paint that was in force where it was written.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Painted {
    pub text: String,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub reverse: bool,
    pub fg: Option<Colour>,
    pub bg: Option<Colour>,
}

/// Remove every escape sequence from a captured screen, keeping the rest.
pub fn strip_ansi(screen: &str) -> String {
    let mut out = String::with_capacity(screen.len());
    walk(screen, |event| match event {
        Event::Text(run) => out.push_str(run),
        Event::Newline => out.push('\n'),
        Event::Sgr(_) => {}
    });
    out
}

/// The same walk with the paint kept: one run per stretch of text drawn in one
/// style, gathered into the row it was drawn on.
///
/// A row is what sits between two newlines, so `n` newlines give `n + 1` rows
/// and a screen ending in one opens a last empty row. That is not a choice of
/// shape: it is the only reading under which [`strip_ansi`] is these rows
/// joined by newlines.
pub fn painted(screen: &str) -> Vec<Vec<Painted>> {
    let mut rows: Vec<Vec<Painted>> = vec![Vec::new()];
    let mut style = Painted::default();
    let mut run = String::new();
    walk(screen, |event| match event {
        Event::Text(text) => run.push_str(text),
        Event::Newline => {
            close(&mut rows, &style, &mut run);
            // Paint does not survive a row boundary: a colour opened on one
            // row and never closed does not bleed into the next.
            style = Painted::default();
            rows.push(Vec::new());
        }
        Event::Sgr(params) => {
            let mut next = style.clone();
            apply(&mut next, params);
            // A parameter that changed nothing — and a real capture is full of
            // them — must not split one run into two.
            if next != style {
                close(&mut rows, &style, &mut run);
                style = next;
            }
        }
    });
    close(&mut rows, &style, &mut run);
    rows
}

/// Put the pending run on the row being built, if it has anything in it. An
/// empty run is dropped, which is what keeps back-to-back escape sequences
/// from littering a row with styled nothings.
fn close(rows: &mut [Vec<Painted>], style: &Painted, run: &mut String) {
    if run.is_empty() {
        return;
    }
    let mut done = style.clone();
    done.text = std::mem::take(run);
    if let Some(row) = rows.last_mut() {
        row.push(done);
    }
}

// ── the walk ────────────────────────────────────────────────────────────────

/// `ESC`, the seven-bit introducer everything below is built on.
const ESC: char = '\u{1b}';

/// `BEL`, which every terminal worth the name takes as a string terminator
/// even though ECMA-48 spells that `ST`.
const BEL: char = '\u{7}';

/// `CSI`, `OSC`, `DCS`, `APC`, `PM`, `SOS` and `ST` in their eight-bit forms.
/// tmux hands U+009B back through the capture verbatim, so these are not
/// theoretical.
const CSI_8: char = '\u{9b}';
const OSC_8: char = '\u{9d}';
const DCS_8: char = '\u{90}';
const APC_8: char = '\u{9f}';
const PM_8: char = '\u{9e}';
const SOS_8: char = '\u{98}';
const ST_8: char = '\u{9c}';

/// What the walk hands a collector. Text arrives borrowed from the screen, so
/// neither collector pays for a copy it does not keep.
enum Event<'a> {
    /// A run of text with no escape and no newline in it.
    Text(&'a str),
    /// A row boundary. Structure rather than text: paint does not cross it.
    Newline,
    /// A `CSI … m` sequence's parameters, without its introducer or its final
    /// byte. `ESC[m` arrives as the empty string.
    Sgr(&'a str),
}

/// What the character just read opened, if anything.
enum Opened {
    /// Nothing: it is text, and it stays in the run being gathered.
    Text,
    /// A row boundary.
    Newline,
    /// A control sequence: parameters, then a final byte in `@`..=`~`.
    Csi,
    /// A control string — OSC, DCS, APC, PM or SOS — running to its `ST`.
    Str,
    /// An escape with no body: `ESC x` for any other `x`, a lone `ESC` at the
    /// end of the screen, or a terminator with nothing open. Read and dropped.
    Nothing,
}

/// The grammar, walked once.
///
/// **An `ESC` inside a control string or a control sequence aborts it and is
/// read again** rather than being swallowed with the character after it. That
/// is what makes a truncated `ESC ] 0 ; title ESC [ 31 m` consume the `CSI`
/// too instead of printing `31m`, and it costs nothing on a well-formed
/// screen, where an `ESC \` read again is just two characters of paint. The
/// one `ESC` is visited twice and the walk then always moves on, so reading a
/// screen stays linear in its length: a capture cut off mid-sequence is the
/// ordinary case here, and a long unterminated string must not cost more than
/// a scan of it.
fn walk(screen: &str, mut emit: impl FnMut(Event<'_>)) {
    let mut i = 0;
    let mut text_from = 0;
    while let Some(c) = screen[i..].chars().next() {
        let here = i;
        i += c.len_utf8();
        let opened = match c {
            '\n' => Opened::Newline,
            ESC => match screen[i..].chars().next() {
                // A lone `ESC` at the end has nothing to open.
                None => Opened::Nothing,
                // An `ESC` cancels the one before it rather than becoming its
                // body, which is the rule the two scans below keep as well, so
                // an `ESC` always begins a sequence and never ends up as
                // somebody's payload. Left where it is, and read by the next
                // turn of this loop.
                Some(after) if after == ESC => Opened::Nothing,
                Some(next) => {
                    i += next.len_utf8();
                    match next {
                        '[' => Opened::Csi,
                        ']' | 'P' | '_' | '^' | 'X' => Opened::Str,
                        // Any other `ESC x` is two characters and both paint.
                        _ => Opened::Nothing,
                    }
                }
            },
            CSI_8 => Opened::Csi,
            OSC_8 | DCS_8 | APC_8 | PM_8 | SOS_8 => Opened::Str,
            // A terminator with no string open closes nothing and is still not
            // text. Its seven-bit twin `ESC \` is read by the rule above, and
            // the point of the eight-bit forms is that both behave alike.
            ST_8 => Opened::Nothing,
            _ => Opened::Text,
        };
        if matches!(opened, Opened::Text) {
            continue;
        }
        if here > text_from {
            emit(Event::Text(&screen[text_from..here]));
        }
        match opened {
            Opened::Newline => emit(Event::Newline),
            Opened::Csi => {
                let (end, params) = scan_csi(screen, i);
                i = end;
                if let Some(params) = params {
                    emit(Event::Sgr(params));
                }
            }
            Opened::Str => i = scan_string(screen, i),
            Opened::Nothing | Opened::Text => {}
        }
        text_from = i;
    }
    if text_from < screen.len() {
        emit(Event::Text(&screen[text_from..]));
    }
}

/// A control sequence from just past its introducer: parameters and
/// intermediates, then a final byte in `@`..=`~`. Answers with where the walk
/// goes on and, where the final byte is `m`, the parameters an SGR wants. One
/// that never ends is read to the end of the screen.
fn scan_csi(screen: &str, from: usize) -> (usize, Option<&str>) {
    let mut i = from;
    while let Some(c) = screen[i..].chars().next() {
        if c == ESC {
            return (i, None); // aborted, and the `ESC` is read again
        }
        let end = i + c.len_utf8();
        if ('\u{40}'..='\u{7e}').contains(&c) {
            return (end, (c == 'm').then(|| &screen[from..i]));
        }
        i = end;
    }
    (screen.len(), None)
}

/// A control string from just past its introducer — OSC, DCS, APC, PM and SOS
/// are all one shape — to its `ST`, which is `ESC \`, `BEL` or U+009C. Answers
/// with where the walk goes on. **One that never ends is read to the end of
/// the screen** rather than having its body printed as text: a capture cut off
/// mid-string is ordinary, and half a DCS payload on somebody's screen is what
/// this exists to prevent.
fn scan_string(screen: &str, from: usize) -> usize {
    let mut i = from;
    while let Some(c) = screen[i..].chars().next() {
        match c {
            ESC => return i, // aborted, and the `ESC` is read again
            BEL | ST_8 => return i + c.len_utf8(),
            _ => i += c.len_utf8(),
        }
    }
    screen.len()
}

// ── the paint ───────────────────────────────────────────────────────────────

/// Fold one `CSI … m` parameter list into the paint in force.
///
/// **Every attribute's off-parameter is read with its on-parameter** — 22, 23,
/// 24, 27, 39 and 49 — because a reader that can open bold and cannot close it
/// draws a capture as something the pane never looked like, which is the whole
/// of the complaint this module answers. Every parameter outside the set is
/// dropped and never reaches a renderer as text: blink, conceal, strike,
/// overline and the rest.
///
/// The parameters are the semicolon-separated form, which is what tmux writes.
/// The colon-separated form of T.416 (`38:2::r:g:b`) reads as one parameter
/// nothing recognises and is dropped rather than misread, which leaks no text
/// either way.
fn apply(style: &mut Painted, params: &str) {
    let mut rest = params.split(';');
    while let Some(param) = rest.next() {
        // A parameter nothing recognises is skipped rather than a reason to
        // stop reading: `1;x;31` is still bold and still red.
        let Some(n) = number(param) else { continue };
        match n {
            // `ESC[0m`, `ESC[m` and an empty parameter all reset: `ESC[m`
            // arrives as one empty parameter, which reads as the 0 ECMA-48
            // says it stands for.
            0 => *style = Painted::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            3 => style.italic = true,
            4 => style.underline = true,
            7 => style.reverse = true,
            22 => {
                style.bold = false;
                style.dim = false;
            }
            23 => style.italic = false,
            24 => style.underline = false,
            27 => style.reverse = false,
            30..=37 => style.fg = Some(Colour::Ansi((n - 30) as u8)),
            38 => {
                if let Some(colour) = extended(&mut rest) {
                    style.fg = Some(colour);
                }
            }
            39 => style.fg = None,
            40..=47 => style.bg = Some(Colour::Ansi((n - 40) as u8)),
            48 => {
                if let Some(colour) = extended(&mut rest) {
                    style.bg = Some(colour);
                }
            }
            49 => style.bg = None,
            90..=97 => style.fg = Some(Colour::Ansi((n - 90 + 8) as u8)),
            100..=107 => style.bg = Some(Colour::Ansi((n - 100 + 8) as u8)),
            _ => {}
        }
    }
}

/// The parameters `38` and `48` take: `5;n` for one of the 256, `2;r;g;b` for
/// a colour given outright. A component that will not read leaves the channel
/// the colour it had.
///
/// **A colour is a fixed number of parameters and stays that many even when
/// one of them will not read**, so all three components come off the list
/// before any of them is allowed to fail. Failing on the red would leave the
/// green and the blue behind to be read as attributes, and `38;2;300;1;4`
/// would turn on bold and underline out of nothing but a bad red.
fn extended<'a>(rest: &mut impl Iterator<Item = &'a str>) -> Option<Colour> {
    match number(rest.next()?)? {
        5 => Some(Colour::Indexed(component(rest.next()?)?)),
        2 => {
            let r = component(rest.next()?);
            let g = component(rest.next()?);
            let b = component(rest.next()?);
            Some(Colour::Rgb(r?, g?, b?))
        }
        _ => None,
    }
}

/// One parameter as a number. Empty is 0, the value ECMA-48 says a parameter
/// left out stands for, which is what makes `ESC[m` and `ESC[;m` resets.
/// Anything that is not a run of digits, or is a run too long to be a
/// parameter, is not a number and its caller drops it rather than guessing.
fn number(param: &str) -> Option<u16> {
    if param.is_empty() {
        return Some(0);
    }
    if !param.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    param.parse().ok()
}

/// One colour component, which has to fit a byte.
fn component(param: &str) -> Option<u8> {
    u8::try_from(number(param)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── the walk ────────────────────────────────────────────────────────────

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

    #[test]
    fn a_control_string_is_removed_body_and_all() {
        for opener in ['P', '_', '^', 'X'] {
            let screen = format!("a\u{1b}{opener}payload;1;2\u{1b}\\b");
            assert_eq!(strip_ansi(&screen), "ab", "ESC {opener}");
        }
    }

    #[test]
    fn a_control_string_that_never_ends_takes_its_body_with_it() {
        assert_eq!(strip_ansi("a\u{1b}Pnever terminated"), "a");
    }

    #[test]
    fn an_eight_bit_introducer_is_a_sequence_like_its_seven_bit_twin() {
        assert_eq!(strip_ansi("a\u{9b}31mb"), "ab");
        assert_eq!(strip_ansi("a\u{9d}0;title\u{9c}b"), "ab");
        assert_eq!(strip_ansi("a\u{9c}b"), "ab");
    }

    #[test]
    fn an_escape_inside_a_sequence_begins_a_new_one() {
        // The `OSC` is cut off by the `CSI`, and the `CSI` is read rather than
        // printed: `31m` is paint, not two characters and a letter.
        assert_eq!(strip_ansi("a\u{1b}]0;title\u{1b}[31mb"), "ab");
    }

    // ── the paint ───────────────────────────────────────────────────────────

    /// The one row of runs a screen with no newline in it makes.
    fn row(screen: &str) -> Vec<Painted> {
        let mut rows = painted(screen);
        assert_eq!(rows.len(), 1, "{screen:?}");
        rows.pop().unwrap_or_default()
    }

    #[test]
    fn every_attribute_is_read_and_every_one_can_be_closed() {
        let runs = row("\u{1b}[1;2;3;4;7mon\u{1b}[22;23;24;27moff");
        assert_eq!(runs.len(), 2, "{runs:?}");
        assert_eq!(
            (
                runs[0].bold,
                runs[0].dim,
                runs[0].italic,
                runs[0].underline,
                runs[0].reverse
            ),
            (true, true, true, true, true)
        );
        assert_eq!(
            (
                runs[1].bold,
                runs[1].dim,
                runs[1].italic,
                runs[1].underline,
                runs[1].reverse
            ),
            (false, false, false, false, false),
            "an attribute that cannot be closed draws a screen the pane never looked like"
        );
    }

    #[test]
    fn colour_is_read_in_all_three_forms_and_on_both_channels() {
        assert_eq!(row("\u{1b}[31mred")[0].fg, Some(Colour::Ansi(1)));
        assert_eq!(row("\u{1b}[91mbright")[0].fg, Some(Colour::Ansi(9)));
        assert_eq!(row("\u{1b}[41mred")[0].bg, Some(Colour::Ansi(1)));
        assert_eq!(row("\u{1b}[101mbright")[0].bg, Some(Colour::Ansi(9)));
        assert_eq!(row("\u{1b}[38;5;208mone")[0].fg, Some(Colour::Indexed(208)));
        assert_eq!(row("\u{1b}[48;5;17mone")[0].bg, Some(Colour::Indexed(17)));
        assert_eq!(
            row("\u{1b}[38;2;10;20;30mrgb")[0].fg,
            Some(Colour::Rgb(10, 20, 30))
        );
        assert_eq!(
            row("\u{1b}[48;2;10;20;30mrgb")[0].bg,
            Some(Colour::Rgb(10, 20, 30))
        );
    }

    #[test]
    fn a_colour_is_closed_without_closing_the_other_channel() {
        let runs = row("\u{1b}[31;42mboth\u{1b}[39mbackground");
        assert_eq!(runs[1].fg, None);
        assert_eq!(runs[1].bg, Some(Colour::Ansi(2)));
    }

    #[test]
    fn an_empty_parameter_list_resets_everything() {
        for reset in ["\u{1b}[0m", "\u{1b}[m"] {
            let runs = row(&format!("\u{1b}[1;31mpainted{reset}plain"));
            assert_eq!(
                runs[1],
                Painted {
                    text: "plain".to_string(),
                    ..Painted::default()
                }
            );
        }
    }

    #[test]
    fn a_bad_component_costs_its_colour_and_nothing_else() {
        // The green and the blue are the colour's, not two attributes to be
        // read once the red has failed.
        let runs = row("\u{1b}[38;2;300;1;4mtext");
        assert_eq!(runs[0].fg, None);
        assert!(!runs[0].bold && !runs[0].underline, "{runs:?}");
    }

    #[test]
    fn a_parameter_nothing_recognises_does_not_stop_the_rest() {
        let runs = row("\u{1b}[1;9;31mtext");
        assert!(runs[0].bold);
        assert_eq!(runs[0].fg, Some(Colour::Ansi(1)));
    }

    #[test]
    fn paint_does_not_bleed_from_one_row_into_the_next() {
        let rows = painted("\u{1b}[1mbold\nplain");
        assert!(rows[0][0].bold);
        assert!(!rows[1][0].bold, "{rows:?}");
    }

    #[test]
    fn a_parameter_that_changes_nothing_does_not_split_a_run() {
        let runs = row("\u{1b}[1mbo\u{1b}[1mld");
        assert_eq!(runs.len(), 1, "{runs:?}");
        assert_eq!(runs[0].text, "bold");
    }

    #[test]
    fn a_row_of_nothing_but_paint_has_no_runs_on_it() {
        let rows = painted("\u{1b}[1m\u{1b}[0m\nword");
        assert!(rows[0].is_empty(), "{rows:?}");
        assert_eq!(rows[1][0].text, "word");
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

    /// A complete, valid escape sequence: one of the shapes the walk knows.
    fn paint_one(out: &mut String, rng: &mut Rng) {
        match rng.below(4) {
            0 => {
                out.push('\u{1b}');
                out.push('[');
                let params = rng.below(4);
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
            2 => {
                const STRINGS: &[char] = &['P', '_', '^', 'X'];
                out.push('\u{1b}');
                out.push(STRINGS[rng.below(STRINGS.len() as u64) as usize]);
                out.push_str("body;1;2\u{1b}\\");
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
    fn painted_over(plain: &str, seed: u64) -> String {
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
            let screen = painted_over(&plain, seed);
            assert_eq!(strip_ansi(&screen), plain, "seed {seed}: {screen:?}");
        }
    }

    #[test]
    fn strip_is_the_painted_rows_with_the_paint_thrown_away() {
        for seed in 0..1000u64 {
            let screen = painted_over(&plain_text(seed), seed);
            let said: Vec<String> = painted(&screen)
                .iter()
                .map(|row| row.iter().map(|run| run.text.as_str()).collect())
                .collect();
            assert_eq!(strip_ansi(&screen), said.join("\n"), "seed {seed}");
        }
    }
}
