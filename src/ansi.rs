//! Reading the escape sequences in a captured screen.
//!
//! tmux's `capture-pane -e` keeps every SGR sequence the pane drew, sometimes
//! mid-word. A rule matching text wants the words alone; the card wants the
//! words and the paint.
//!
//! [`strip_ansi`], [`painted`] and [`laid_out`] are three collectors over one
//! private [`walk`], so the grammar lives in one place:
//!
//! - `strip_ansi(s)` is `painted(s)` with the style dropped and the rows joined
//!   by newlines. A property test at the bottom of this file holds them to it.
//! - `laid_out` also applies cursor motion, for output that draws rather than
//!   streams.
//!
//! This removes escapes. It is not a display sanitiser: characters a terminal
//! must not be handed are [`crate::tmux::sanitize`]'s job, applied after this.

/// A colour in the three forms SGR carries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Colour {
    /// SGR 30-37 and 90-97 (foreground) or 40-47 and 100-107 (background), as
    /// the ANSI index 0-15.
    Ansi(u8),
    /// `38;5;n` and `48;5;n`.
    Indexed(u8),
    /// `38;2;r;g;b` and `48;2;r;g;b`.
    Rgb(u8, u8, u8),
}

/// A run of text and the style in force where it was written.
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

/// Removes every escape sequence from a captured screen.
pub fn strip_ansi(screen: &str) -> String {
    let mut out = String::with_capacity(screen.len());
    walk(screen, |event| match event {
        Event::Text(run) => out.push_str(run),
        Event::Newline => out.push('\n'),
        Event::Control { .. } => {}
    });
    out
}

/// The screen as rows of styled runs, one run per stretch of text in one
/// style.
///
/// `n` newlines give `n + 1` rows, so a screen ending in a newline has an
/// empty last row. That is what makes [`strip_ansi`] equal these rows joined
/// by newlines.
pub fn painted(screen: &str) -> Vec<Vec<Painted>> {
    let mut rows: Vec<Vec<Painted>> = vec![Vec::new()];
    let mut style = Painted::default();
    let mut run = String::new();
    walk(screen, |event| match event {
        Event::Text(text) => run.push_str(text),
        Event::Newline => {
            close(&mut rows, &style, &mut run);
            // The style stays in force across rows, as on a terminal. tmux only
            // writes an escape where an attribute changes, so a box three rows
            // tall opens its background once on the first row.
            rows.push(Vec::new());
        }
        Event::Control { params, end: 'm' } => {
            let mut next = style.clone();
            apply(&mut next, params);
            // Real captures are full of no-op parameters; they must not split
            // a run.
            if next != style {
                close(&mut rows, &style, &mut run);
                style = next;
            }
        }
        // Motion means nothing to a collector that keeps runs in write order.
        Event::Control { .. } => {}
    });
    close(&mut rows, &style, &mut run);
    rows
}

/// Pushes the pending run onto the current row. Empty runs are dropped, so
/// back-to-back escapes do not leave styled empty runs behind.
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

/// The screen as a terminal would show it, with cursor motion applied.
///
/// [`strip_ansi`] keeps characters in arrival order, which is wrong for output
/// that positions its cursor before each word: `Accessing` and `workspace:`
/// come out as `Accessingworkspace:`. This lays the walk out on a [`Grid`] and
/// returns the last character drawn in each cell.
///
/// Handles CR, LF, BS, CUP (`H`, `f`), CUU/CUD/CUF/CUB (`A`-`D`), CHA (`G`),
/// VPA (`d`), EL (`K`) and ED (`J`). Everything else draws no cell and is
/// skipped, as in [`strip_ansi`].
pub fn laid_out(raw: &str) -> String {
    let mut grid = Grid::new(raw.len());
    walk(raw, |event| match event {
        Event::Text(text) => grid.write(text),
        Event::Newline => grid.newline(),
        Event::Control { params, end } => grid.moved(params, end),
    });
    grid.read()
}

// The walk.

/// `ESC`.
const ESC: char = '\u{1b}';

/// `BS`, a character that moves the cursor.
const BS: char = '\u{8}';

/// `BEL`, which terminals accept as a string terminator in place of `ST`.
const BEL: char = '\u{7}';

/// `CSI`, `OSC`, `DCS`, `APC`, `PM`, `SOS` and `ST` in their eight-bit forms.
/// tmux passes U+009B through a capture verbatim.
const CSI_8: char = '\u{9b}';
const OSC_8: char = '\u{9d}';
const DCS_8: char = '\u{90}';
const APC_8: char = '\u{9f}';
const PM_8: char = '\u{9e}';
const SOS_8: char = '\u{98}';
const ST_8: char = '\u{9c}';

/// What the walk hands a collector. Text is borrowed from the screen.
enum Event<'a> {
    /// Text with no escape and no newline in it.
    Text(&'a str),
    /// A row boundary. The style in force carries across it.
    Newline,
    /// A complete control sequence: its parameters without the introducer,
    /// and its final byte. `ESC[m` arrives as `""` and `'m'`.
    Control { params: &'a str, end: char },
}

/// What the character just read opened.
enum Opened {
    /// Nothing: it is text.
    Text,
    /// A row boundary.
    Newline,
    /// A control sequence: parameters, then a final byte in `@`..=`~`.
    Csi,
    /// A control string (OSC, DCS, APC, PM or SOS) running to its `ST`.
    Str,
    /// An escape with no body: `ESC x` for any other `x`, a lone trailing
    /// `ESC`, or a terminator with nothing open. Dropped.
    Nothing,
}

/// Walks the escape grammar once, emitting text, newlines and control
/// sequences.
///
/// An `ESC` inside a control string or sequence aborts it and is read again as
/// the start of the next one, so a truncated `ESC ] 0 ; title ESC [ 31 m`
/// still consumes the `CSI` instead of printing `31m`. Each `ESC` is visited at
/// most twice, so the walk stays linear even on long unterminated strings.
fn walk(screen: &str, mut emit: impl FnMut(Event<'_>)) {
    let mut i = 0;
    let mut text_from = 0;
    while let Some(c) = screen[i..].chars().next() {
        let here = i;
        i += c.len_utf8();
        let opened = match c {
            '\n' => Opened::Newline,
            ESC => match screen[i..].chars().next() {
                // A lone `ESC` at the end opens nothing.
                None => Opened::Nothing,
                // An `ESC` cancels the one before it and is read on the next
                // turn of the loop, so an `ESC` never ends up inside a payload.
                Some(after) if after == ESC => Opened::Nothing,
                Some(next) => {
                    i += next.len_utf8();
                    match next {
                        '[' => Opened::Csi,
                        ']' | 'P' | '_' | '^' | 'X' => Opened::Str,
                        // Any other `ESC x` is two characters, both dropped.
                        _ => Opened::Nothing,
                    }
                }
            },
            CSI_8 => Opened::Csi,
            OSC_8 | DCS_8 | APC_8 | PM_8 | SOS_8 => Opened::Str,
            // A stray terminator closes nothing and is not text, like its
            // seven-bit form `ESC \` above.
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
                let (goes_on, sequence) = scan_csi(screen, i);
                i = goes_on;
                if let Some((params, end)) = sequence {
                    emit(Event::Control { params, end });
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

/// Scans a control sequence from just past its introducer to a final byte in
/// `@`..=`~`. Returns where the walk resumes and, if the sequence finished,
/// its parameters and final byte. An unfinished sequence runs to the end of
/// the screen and yields nothing.
fn scan_csi(screen: &str, from: usize) -> (usize, Option<(&str, char)>) {
    let mut i = from;
    while let Some(c) = screen[i..].chars().next() {
        if c == ESC {
            return (i, None); // aborted, and the `ESC` is read again
        }
        let end = i + c.len_utf8();
        if ('\u{40}'..='\u{7e}').contains(&c) {
            return (end, Some((&screen[from..i], c)));
        }
        i = end;
    }
    (screen.len(), None)
}

/// Scans a control string (OSC, DCS, APC, PM, SOS) from just past its
/// introducer to its `ST` (`ESC \`, `BEL` or U+009C) and returns where the
/// walk resumes. An unterminated string runs to the end of the screen, so a
/// capture cut mid-payload never prints the payload.
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

// The paint.

/// Applies one `CSI ... m` parameter list to the style in force.
///
/// Every attribute's off parameter (22, 23, 24, 27, 39, 49) is handled with
/// its on parameter, so nothing stays bold that the pane closed. Other
/// parameters (blink, strike and the rest) are dropped.
///
/// Only the semicolon form tmux writes is parsed. The colon form of T.416
/// (`38:2::r:g:b`) reads as one unknown parameter and is dropped.
fn apply(style: &mut Painted, params: &str) {
    let mut rest = params.split(';');
    while let Some(param) = rest.next() {
        // An unknown parameter is skipped: `1;x;31` is still bold and red.
        let Some(n) = number(param) else { continue };
        match n {
            // `ESC[m` arrives as one empty parameter, which reads as 0.
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

/// Reads the colour after `38` or `48`: `5;n` for the 256-colour table,
/// `2;r;g;b` for RGB. A component that fails leaves the channel as it was.
///
/// All three RGB components are consumed before any may fail, so a bad red in
/// `38;2;300;1;4` does not leave `1` and `4` to be read as bold and underline.
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

/// One parameter as a number. Empty is 0, as ECMA-48 defines an omitted
/// parameter, which makes `ESC[m` and `ESC[;m` resets. Anything but a short
/// run of digits is `None`.
fn number(param: &str) -> Option<u16> {
    if param.is_empty() {
        return Some(0);
    }
    if !param.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    param.parse().ok()
}

/// One colour component, which must fit a byte.
fn component(param: &str) -> Option<u8> {
    u8::try_from(number(param)?).ok()
}

// The grid.

/// Cells a capture may fill beyond one per byte; see [`Grid`].
const SLACK: usize = 64 * 1024;

/// The cells a drawing has filled, and the cursor filling them.
///
/// No width or height is assumed: a cell addressed past the end of a row or
/// the last row grows the grid. What it holds is capped at one cell per
/// capture byte plus [`SLACK`], since only characters from the capture fill
/// cells. Draws beyond the cap are dropped, so `ESC[999999999;999999999H`
/// cannot allocate a gigabyte of rows.
struct Grid {
    rows: Vec<Vec<char>>,
    row: usize,
    col: usize,
    /// Cells held: every row's cells, plus one per row.
    held: usize,
    /// The most it may hold.
    cap: usize,
}

impl Grid {
    /// An empty grid for a capture of `capture` bytes.
    fn new(capture: usize) -> Grid {
        Grid {
            rows: Vec::new(),
            row: 0,
            col: 0,
            held: 0,
            cap: capture + SLACK,
        }
    }

    /// Draws text, one character per cell. CR returns to the first column and
    /// BS steps one cell left.
    fn write(&mut self, text: &str) {
        for c in text.chars() {
            match c {
                '\r' => self.col = 0,
                BS => self.col = self.col.saturating_sub(1),
                _ => self.put(c),
            }
        }
    }

    /// A newline: down a row and back to the first column.
    ///
    /// A terminal's LF keeps the column, but pty captures already have a CR
    /// before every LF. Files written without a pty do not, and keeping the
    /// column there would walk each line diagonally.
    fn newline(&mut self) {
        self.row = self.row.saturating_add(1);
        self.col = 0;
    }

    /// One character at the cursor, growing the row and cells before it as
    /// blanks. The cursor advances whether or not the cell fit.
    fn put(&mut self, c: char) {
        // A cursor saturated at usize::MAX has no cell to fill.
        if let (Some(tall), Some(wide)) = (self.row.checked_add(1), self.col.checked_add(1)) {
            let rows = tall.saturating_sub(self.rows.len());
            let width = self.rows.get(self.row).map_or(0, Vec::len);
            let cells = wide.saturating_sub(width);
            if self.held.saturating_add(rows).saturating_add(cells) <= self.cap {
                self.held += rows + cells;
                if self.rows.len() < tall {
                    self.rows.resize_with(tall, Vec::new);
                }
                let row = &mut self.rows[self.row];
                if row.len() < wide {
                    row.resize(wide, ' ');
                }
                row[self.col] = c;
            }
        }
        self.col = self.col.saturating_add(1);
    }

    /// Applies cursor motion and erasure. Private-parameter sequences and
    /// unknown final bytes are ignored.
    fn moved(&mut self, params: &str, end: char) {
        if params.starts_with(['<', '=', '>', '?']) {
            return;
        }
        match end {
            'H' | 'f' => {
                self.row = at(params, 0);
                self.col = at(params, 1);
            }
            'A' => self.row = self.row.saturating_sub(count(params, 0)),
            'B' => self.row = self.row.saturating_add(count(params, 0)),
            'C' => self.col = self.col.saturating_add(count(params, 0)),
            'D' => self.col = self.col.saturating_sub(count(params, 0)),
            'G' => self.col = at(params, 0),
            'd' => self.row = at(params, 0),
            'K' => match mode(params) {
                0 => self.cut(self.row, self.col),
                1 => self.blank(self.row, self.col),
                2 => self.cut(self.row, 0),
                _ => {}
            },
            // Rows below the cursor are dropped. Rows above it are emptied but
            // kept, so the rows under them stay where they were drawn.
            'J' => match mode(params) {
                0 => {
                    self.cut(self.row, self.col);
                    self.keep_rows(self.row.saturating_add(1));
                }
                1 => {
                    self.blank(self.row, self.col);
                    for row in 0..self.row.min(self.rows.len()) {
                        self.cut(row, 0);
                    }
                }
                2 | 3 => self.keep_rows(0),
                _ => {}
            },
            _ => {}
        }
    }

    /// Drops a row's cells from `from` on.
    fn cut(&mut self, row: usize, from: usize) {
        if let Some(cells) = self.rows.get_mut(row) {
            self.held -= cells.len().saturating_sub(from);
            cells.truncate(from);
        }
    }

    /// Blanks a row up to and including `to`. The cells past it keep their
    /// place, so none are released.
    fn blank(&mut self, row: usize, to: usize) {
        if let Some(cells) = self.rows.get_mut(row) {
            for cell in cells.iter_mut().take(to.saturating_add(1)) {
                *cell = ' ';
            }
        }
    }

    /// Drops every row from `keep` on.
    fn keep_rows(&mut self, keep: usize) {
        while self.rows.len() > keep {
            if let Some(cells) = self.rows.pop() {
                self.held -= cells.len() + 1;
            }
        }
    }

    /// The grid as text, one row per line, without trailing blanks on each row
    /// or trailing empty rows.
    fn read(&self) -> String {
        let mut rows: Vec<String> = self
            .rows
            .iter()
            .map(|cells| {
                let mut line: String = cells.iter().collect();
                line.truncate(line.trim_end_matches(' ').len());
                line
            })
            .collect();
        while rows.last().is_some_and(String::is_empty) {
            rows.pop();
        }
        rows.join("\n")
    }
}

/// The `i`th parameter as a count or a 1-based position. Missing, empty, zero
/// or unparsable reads as 1, the ECMA-48 default.
fn count(params: &str, i: usize) -> usize {
    params
        .split(';')
        .nth(i)
        .and_then(|param| param.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(1)
}

/// The `i`th parameter as a 0-based row or column.
fn at(params: &str, i: usize) -> usize {
    count(params, i) - 1
}

/// The mode of an erase sequence, 0 when omitted.
fn mode(params: &str) -> usize {
    params
        .split(';')
        .next()
        .and_then(|param| param.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The walk.

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
        // The `OSC` is cut off by the `CSI`, which is read as paint, not
        // printed as `31m`.
        assert_eq!(strip_ansi("a\u{1b}]0;title\u{1b}[31mb"), "ab");
    }

    // The paint.

    /// The runs of a screen with no newline in it.
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
        // The green and blue belong to the colour, not to two attributes read
        // after the red failed.
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
    fn paint_left_open_on_one_row_is_still_in_force_on_the_next() {
        // pi 0.85.1's user-message box opens its background on the padding
        // row above the text and closes it two rows later. The text row
        // between carries no escape at all.
        let rows = painted("\u{1b}[48;2;33;34;47m    \n text\n    \u{1b}[49m\nplain");
        let bg = Some(Colour::Rgb(33, 34, 47));
        assert_eq!(rows[0][0].bg, bg);
        assert_eq!(rows[1][0].bg, bg, "{rows:?}");
        assert_eq!(rows[2][0].bg, bg, "{rows:?}");
        assert_eq!(rows[3][0].bg, None, "{rows:?}");
        // And the same for an attribute.
        let rows = painted("\u{1b}[1mbold\nstill");
        assert!(rows[1][0].bold, "{rows:?}");
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

    // The grid.

    /// Two frames of a boot drawn with cursor positioning: each word placed by
    /// moving the cursor, and the second frame drawn over the first. Modelled
    /// on a claude boot that read as `Accessingworkspace:`.
    const BOOT: &str = concat!(
        // A title, a word further along the same row, and a status row.
        "\u{1b}[2J\u{1b}[H",
        "\u{1b}[1;1HAccessing",
        "\u{1b}[1;17Hworkspace:",
        "\u{1b}[2;3Hreading the files",
        // The second frame: the status row redrawn shorter, the leftover
        // erased.
        "\u{1b}[2;3Hready\u{1b}[K",
        // The title row finished with a row address, a column address and a
        // move right.
        "\u{1b}[1d\u{1b}[1G\u{1b}[27Cin ~/Sites",
    );

    #[test]
    fn a_boot_drawn_with_motion_reads_as_the_last_frame_of_it() {
        assert_eq!(
            laid_out(BOOT),
            "Accessing       workspace: in ~/Sites\n  ready",
            "the cells the cursor skipped are spaces, not nothing"
        );
        assert_eq!(
            strip_ansi(BOOT),
            "Accessingworkspace:reading the filesreadyin ~/Sites",
            "which is the reading this exists to replace"
        );
    }

    #[test]
    fn a_return_draws_over_the_row_it_returns_to() {
        assert_eq!(laid_out(" 1/3\r 2/3\r 3/3 done\r\nlast"), " 3/3 done\nlast");
        // A shorter draw leaves the tail of the longer one, as on a terminal.
        assert_eq!(laid_out("loading......\rdone"), "doneing......");
        // A backspace steps back one cell without erasing it.
        assert_eq!(laid_out("ab\u{8}c"), "ac");
    }

    #[test]
    fn the_cursor_walks_every_way_the_grid_knows() {
        // Down two, right three, up one, left one, then a word.
        assert_eq!(laid_out("\u{1b}[2B\u{1b}[3C\u{1b}[A\u{1b}[Dxy"), "\n  xy");
        // Row and column addresses are 1-based.
        assert_eq!(laid_out("\u{1b}[3d\u{1b}[5Gx"), "\n\n    x");
        // An omitted parameter is 1.
        assert_eq!(laid_out("a\u{1b}[Bb"), "a\n b");
    }

    #[test]
    fn erasing_takes_the_cells_with_it() {
        // To the end of the row, from its start, and all of it.
        assert_eq!(laid_out("abcdef\u{1b}[1;4H\u{1b}[K"), "abc");
        assert_eq!(laid_out("abcdef\u{1b}[1;3H\u{1b}[1K"), "   def");
        assert_eq!(laid_out("abcdef\u{1b}[2K"), "");
        // The screen from the cursor down, up to the cursor, and all of it.
        assert_eq!(laid_out("one\ntwo\nthree\u{1b}[2;2H\u{1b}[J"), "one\nt");
        assert_eq!(
            laid_out("one\ntwo\nthree\u{1b}[2;2H\u{1b}[1J"),
            "\n  o\nthree"
        );
        assert_eq!(laid_out("one\ntwo\u{1b}[2Jafter"), "\n   after");
    }

    #[test]
    fn nothing_that_draws_no_cell_reaches_the_grid() {
        // Paint, a private mode, a control string and an unknown sequence.
        assert_eq!(laid_out("\u{1b}[1;31mred\u{1b}[0m"), "red");
        assert_eq!(laid_out("\u{1b}[?25lhidden\u{1b}[?25h"), "hidden");
        assert_eq!(laid_out("\u{1b}]0;title\u{7}text"), "text");
        assert_eq!(laid_out("a\u{1b}[5Zb"), "ab");
        // A private parameter string is a mode, whatever its final byte.
        assert_eq!(laid_out("a\u{1b}[?3;4Hb"), "ab");
    }

    #[test]
    fn a_reading_ends_at_the_last_cell_with_anything_in_it() {
        assert_eq!(laid_out("one\ntwo\n"), "one\ntwo");
        assert_eq!(laid_out("row   \u{1b}[10Cx\u{1b}[1;5H\u{1b}[K"), "row");
        assert_eq!(laid_out("\n\n\n"), "");
        assert_eq!(laid_out(""), "");
    }

    #[test]
    fn a_cursor_walked_to_the_end_of_the_address_space_does_not_panic() {
        let far = "18446744073709551615";
        assert_eq!(laid_out(&format!("ab\u{1b}[{far}Bx")), "ab");
        assert_eq!(laid_out(&format!("ab\u{1b}[{far}Cx")), "ab");
        assert_eq!(laid_out(&format!("ab\u{1b}[{far}C\u{1b}[1K")), "");
        assert_eq!(laid_out(&format!("ab\ncd\u{1b}[{far}B\u{1b}[J")), "ab\ncd");
        // Back from there onto cells the capture could fill.
        assert_eq!(
            laid_out(&format!("\u{1b}[{far}C\u{1b}[{far}C\u{1b}[1Gx")),
            "x"
        );
    }

    #[test]
    fn a_jump_further_than_the_capture_could_fill_costs_nothing() {
        // A position further out than the whole capture could fill: the cell
        // is dropped instead of the grid growing to reach it. Repeated 32k
        // times, so growing would not finish.
        let hostile = "\u{1b}[999999999;999999999Hx".repeat(32 * 1024);
        assert_eq!(laid_out(&hostile), "");
        // Drawing resumes once the cursor is back in range.
        assert_eq!(laid_out("\u{1b}[999999;999999Hgone\u{1b}[1;1Hhere"), "here");
    }

    #[test]
    fn the_grid_survives_the_hostile_fixtures() {
        for screen in FIXTURES {
            let out = laid_out(screen);
            assert!(!out.contains('\u{1b}'), "an escape reached {screen:?}");
            for end in 0..=screen.len() {
                if screen.is_char_boundary(end) {
                    laid_out(&screen[..end]);
                }
            }
        }
    }

    // Where the walk knowingly differs from a real terminal. These are
    // decisions, not bugs.

    /// Any other escape is exactly two characters, which keeps the walk
    /// linear. The cost is three-character escapes: `ESC ( B` leaves its `B`
    /// behind as text. The leftover is printable, never an escape.
    #[test]
    fn a_three_character_escape_leaves_its_final_byte_behind() {
        assert_eq!(strip_ansi("a\u{1b}(Bb"), "aBb");
        assert_eq!(strip_ansi("a\u{1b})0b"), "a0b");
    }

    /// The same rule from the other side: `ESC` right before a newline takes
    /// the newline as its second character and the rows join. A terminal would
    /// execute the `LF`.
    #[test]
    fn an_escape_before_a_newline_consumes_the_newline() {
        assert_eq!(strip_ansi("one\u{1b}\ntwo"), "onetwo");
        assert_eq!(painted("one\u{1b}\ntwo"), vec![vec![plain_run("onetwo")]]);
    }

    /// U+0085 and U+0099 are control characters a terminal must not be handed,
    /// but removing them is the display sanitiser's job, not this walk's.
    #[test]
    fn an_unnamed_c1_character_is_left_for_the_display_law() {
        assert_eq!(strip_ansi("a\u{85}b\u{99}c"), "a\u{85}b\u{99}c");
    }

    // Hostile input: bytes off another program's pane must not panic and must
    // cost no more than a scan.

    /// Captures covering what has gone wrong on real panes: cancellations, bad
    /// colour specs, truncations, eight-bit introducers, strays, and a vendor
    /// footer.
    const FIXTURES: [&str; 24] = [
        "a\u{1b}\u{1b}[31mone escape cancelling another",
        "\u{1b}[1m\u{1b}[38;2;300;1;4ma colour spec with a bad red",
        "",
        "no escapes at all",
        "one\ntwo\nthree\n",
        "\n\n\n",
        "\u{1b}[1mbold\u{1b}[0m and \u{1b}[3;4mmore\u{1b}[m",
        "\u{1b}[38;5;196mindexed\u{1b}[48;2;1;2;3mon rgb\u{1b}[0m",
        "\u{1b}[31mred\n\u{1b}[32mgreen\n",
        "\u{1b}]0;a window title\u{7}after the bell",
        "\u{1b}]8;;https://example\u{1b}\\a link\u{1b}]8;;\u{1b}\\",
        "\u{1b}Pq#0;2;0;0;0#0~~\u{1b}\\a sixel just went by",
        "\u{1b}_Gf=100,a=T;base64ish\u{1b}\\a graphic just went by",
        "\u{1b}^a private message\u{1b}\\",
        "\u{1b}Xstart of string\u{1b}\\",
        "\u{9b}1mbold through a C1 introducer\u{9b}0m",
        "\u{9d}0;title\u{9c}\u{90}dcs\u{9c}\u{9f}apc\u{9c}\u{9e}pm\u{9c}\u{98}sos\u{9c}",
        "truncated mid-string \u{1b}]0;never terminated",
        "truncated mid-sequence \u{1b}[38;5",
        "a lone trailing escape \u{1b}",
        "an aborted string \u{1b}]0;title\u{1b}[31m then a CSI",
        "\u{1b}[1mé unicodé ✓ 日本語\u{1b}[0m",
        "\u{9c} a stray terminator, and \u{85} a C1 this walk does not name",
        "  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents\n",
    ];

    /// Every introducer the walk knows, in both widths.
    const INTRODUCERS: [(&str, &str); 15] = [
        ("CSI", "\u{1b}[31;1mtext\u{1b}[0m"),
        ("OSC, BEL-terminated", "\u{1b}]0;window title\u{7}"),
        ("OSC, ST-terminated", "\u{1b}]8;;https://example\u{1b}\\"),
        ("DCS", "\u{1b}Pq#0;2;0;0;0#0~~\u{1b}\\"),
        ("APC", "\u{1b}_Gf=100,a=T;payload\u{1b}\\"),
        ("PM", "\u{1b}^status line\u{1b}\\"),
        ("SOS", "\u{1b}Xstart of string\u{1b}\\"),
        ("any other ESC x", "\u{1b}c"),
        ("CSI, 8-bit", "\u{9b}31;1mtext\u{9b}0m"),
        ("OSC, 8-bit", "\u{9d}0;window title\u{9c}"),
        ("DCS, 8-bit", "\u{90}q#0;2;0;0;0\u{9c}"),
        ("APC, 8-bit", "\u{9f}payload\u{9c}"),
        ("PM, 8-bit", "\u{9e}status line\u{9c}"),
        ("SOS, 8-bit", "\u{98}start of string\u{9c}"),
        ("a stray ST", "\u{9c}"),
    ];

    /// A run with no style.
    fn plain_run(text: &str) -> Painted {
        Painted {
            text: text.into(),
            ..Painted::default()
        }
    }

    /// The style of the first run of the first row.
    fn first(screen: &str) -> Painted {
        painted(screen).swap_remove(0).swap_remove(0)
    }

    /// [`painted`] with the style dropped and the runs joined, written out so
    /// the tests assert the contract rather than the implementation.
    fn flattened(screen: &str) -> String {
        painted(screen)
            .iter()
            .map(|line| line.iter().map(|run| run.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn strip_and_painted_agree_over_the_hostile_fixtures() {
        for screen in FIXTURES {
            assert_eq!(strip_ansi(screen), flattened(screen), "{screen:?}");
        }
    }

    #[test]
    fn the_walk_never_grows_what_it_was_given() {
        for screen in FIXTURES {
            assert!(
                strip_ansi(screen).len() <= screen.len(),
                "the walk grew {screen:?}"
            );
        }
    }

    #[test]
    fn a_truncated_sequence_at_every_length_is_survivable() {
        // Every prefix of every fixture: a capture cut at any byte, as a tail
        // read off a live pane is.
        for screen in FIXTURES {
            for end in 0..=screen.len() {
                if !screen.is_char_boundary(end) {
                    continue;
                }
                let cut = &screen[..end];
                assert_eq!(
                    strip_ansi(cut),
                    flattened(cut),
                    "cut of {screen:?} at {end}"
                );
            }
        }
    }

    #[test]
    fn a_multibyte_character_next_to_an_introducer_does_not_panic() {
        assert_eq!(strip_ansi("\u{1b}é"), ""); // ESC x, where x is two bytes
        assert_eq!(strip_ansi("\u{1b}[é1mtext"), "text"); // inside the parameters
        assert_eq!(strip_ansi("\u{1b}]0;日本語\u{7}kept"), "kept"); // inside a string
        assert_eq!(strip_ansi("\u{9b}日1mtext"), "text"); // after a C1 introducer
    }

    #[test]
    fn a_long_unterminated_string_costs_one_scan_and_no_more() {
        // 128 KiB of body with no terminator. A quadratic walk would not
        // finish in time.
        let hostile = format!("keep me\u{1b}]0;{}", "A".repeat(128 * 1024));
        assert_eq!(strip_ansi(&hostile), "keep me");
        assert_eq!(flattened(&hostile), "keep me");

        // A sequence that never finds its final byte, and an absurdly long
        // parameter list.
        let unfinished = format!("keep me\u{1b}[{}", "1;".repeat(64 * 1024));
        assert_eq!(strip_ansi(&unfinished), "keep me");
        let huge = format!("\u{1b}[{}mtext", "9".repeat(64 * 1024));
        assert_eq!(strip_ansi(&huge), "text");
        assert_eq!(first(&huge), plain_run("text")); // unreadable, so ignored
    }

    #[test]
    fn a_capture_that_is_all_paint_yields_no_runs_at_all() {
        // Many style changes and no text: no runs at all, not empty ones.
        let all_paint = "\u{1b}[1m\u{1b}[0m".repeat(4096);
        assert_eq!(strip_ansi(&all_paint), "");
        assert_eq!(painted(&all_paint), vec![vec![]]);
    }

    #[test]
    fn no_escape_character_reaches_the_output() {
        for (name, sequence) in INTRODUCERS {
            let out = strip_ansi(&format!("<{sequence}>"));
            assert!(
                !out.contains('\u{1b}'),
                "{name}: an escape reached the output of {sequence:?}: {out:?}"
            );
            // The CSI cases carry `text` between two sequences; the rest carry
            // nothing. The sentinels survive and no body byte does.
            let expected = if name.starts_with("CSI") {
                "<text>"
            } else {
                "<>"
            };
            assert_eq!(out, expected, "{name}: {sequence:?}");
        }
    }

    /// A small deterministic generator, so the property tests are
    /// reproducible without a crate.
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

    /// Plain text a pane could hold, with enough non-ASCII to show the walk
    /// counts characters, not bytes. Never `ESC`.
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

    /// One complete escape sequence of a shape the walk knows.
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

    /// `plain` with escape sequences dropped in before some characters and
    /// sometimes after the last.
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
