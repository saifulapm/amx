//! The board's key table: what the view does with a read of stdin.
//!
//! Split out of [`super`] on the day it was written (R-M1-3, and the rule this
//! milestone has applied four times now that no split waits for the hard
//! limit): the parent is the view's *state* and its lifecycle, this is what one
//! key press does to it, and the two change for different reasons.
//!
//! # The keymap is D15's, with the one substitution it forces
//!
//! `↑`/`↓` move, `Enter` jumps, `Esc` closes the peek and then the view, typing
//! filters, `ctrl+s` regroups, `ctrl+b` shows only what is blocked, `ctrl+p`
//! prompts, `ctrl+x` kills on the second press and `ctrl+r` renames. The picker
//! primitive moves its own selection with `ctrl+n`/`ctrl+p`; here `ctrl+p` is
//! D15's prompt key, so this surface reads the arrows instead — which is what
//! D15's own table says it does, and the reason [`decode`] exists.
//!
//! Nothing in this table is reachable while the view is closed, and the prefix
//! key is not reachable while it is open: a chrome surface owns the read, as the
//! picker and copy mode already do. `Esc` is the way out.

use std::io::Write;
use std::os::fd::AsFd;

use amx_core::Effect;
use amx_proto::control::agent::PromptWait;
use amx_proto::control::{Call, agent as agent_proto, pane as pane_proto, workspace};

use super::{AgentsUi, App, Entry, rows};
use crate::input::{Chrome, InputEvent};

/// One key the view's own table reads.
///
/// A deliberately tiny vocabulary, and the module doc's fence on it: this is a
/// *chrome* surface's key table, not the byte path to a pane, so recognising two
/// escape sequences here does not put a lossy key decoder anywhere near the
/// bytes an application receives (04 §7's rule is about that path — see
/// [`crate::input`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Key {
    /// `↑`, in either cursor-key mode.
    Up,
    /// `↓`, in either cursor-key mode.
    Down,
    /// `Enter`.
    Enter,
    /// `Esc`, alone: a sequence this table does not know is swallowed whole
    /// rather than mistaken for one.
    Esc,
    /// Backspace or delete.
    Back,
    /// The space bar, which peeks rather than typing.
    Space,
    /// A control byte, by the byte itself.
    Ctrl(u8),
    /// A printable character, which filters or types.
    Char(u8),
}

/// `ctrl+b`: blocked-only filter.
const CTRL_B: u8 = 0x02;
/// `ctrl+p`: prompt the selected agent.
const CTRL_P: u8 = 0x10;
/// `ctrl+r`: rename the selected agent.
const CTRL_R: u8 = 0x12;
/// `ctrl+s`: switch grouping.
const CTRL_S: u8 = 0x13;
/// `ctrl+x`: kill the selected pane, on the second press.
const CTRL_X: u8 = 0x18;
/// `Esc`.
const ESC: u8 = 0x1b;
/// Delete, which a terminal may send for backspace.
const DEL: u8 = 0x7f;

impl<Fd: AsFd, W: Write> App<Fd, W> {
    /// Route a read of stdin to the open view.
    ///
    /// Through [`crate::input::Input::feed_chrome`] for the reason the picker
    /// and copy mode are (X13's F-C): an SGR report's bytes are `ESC`, `[`, `<`,
    /// digits and `M`, which this table would read as close-the-view followed by
    /// junk. Every report is taken out of the stream there, and the wheel turn
    /// that survives it is dropped here — D14 licenses exactly one interpreted
    /// mouse event, copy-mode scroll, and a wheel that moved a list selection
    /// would be a second one.
    #[must_use]
    pub(in crate::app) fn agents_input(
        &mut self,
        bytes: &[u8],
        sink: &mut impl FnMut(InputEvent<'_>),
    ) -> Effect {
        let mut pieces = self.input.take_chrome();
        self.input.feed_chrome(bytes, &mut pieces);
        let mut keys = std::mem::take(&mut self.agents.keys);
        keys.clear();
        for &piece in &pieces {
            if let Chrome::Keys { start, end } = piece {
                decode(&bytes[start..end], &mut keys);
            }
        }
        self.input.put_chrome(pieces);
        for &key in &keys {
            // A key that closed the view ends the read: the bytes behind it were
            // typed at a board that is no longer there, and letting them fall
            // through to the pane would send a stray `q` to a shell.
            if !self.agents.open {
                break;
            }
            self.agents_key(key, sink);
        }
        keys.clear();
        self.agents.keys = keys;
        Effect::Full
    }

    /// One decoded key.
    fn agents_key(&mut self, key: Key, sink: &mut impl FnMut(InputEvent<'_>)) {
        // Every key but a second `ctrl+x` disarms the kill: the confirmation is
        // "press it again", so anything in between is the user doing something
        // else and must not leave a pane one keystroke from closing.
        if key != Key::Ctrl(CTRL_X) {
            self.agents.armed = None;
        }
        if self.agents.entry.is_some() {
            self.agents_entry_key(key, sink);
            return;
        }
        match key {
            Key::Up => self.agents.step(-1),
            Key::Down => self.agents.step(1),
            Key::Enter => self.agents_enter(sink),
            Key::Esc => {
                // Peek first, then the view: `Esc` closes what is on top of what
                // (D15's own wording), so a user who peeked and changed their
                // mind does not lose the board with it.
                if self.agents.peek.is_some() {
                    self.agents.peek = None;
                } else {
                    self.agents.open = false;
                }
            }
            // A toggle over the selected agent. A collapsed run has no pane to
            // show, so `Space` on one asks for nothing rather than for the last
            // agent the cursor happened to pass.
            Key::Space => {
                self.agents.peek = match self.agents.peek {
                    Some(_) => None,
                    None => self.agents.selected_pane(),
                };
            }
            Key::Back => {
                self.agents.picker.key(DEL);
                self.agents.rebuild();
            }
            Key::Char(byte) => {
                self.agents.picker.key(byte);
                self.agents.rebuild();
            }
            Key::Ctrl(CTRL_S) => {
                self.agents.grouping = self.agents.grouping.other();
                self.agents.rebuild();
            }
            Key::Ctrl(CTRL_B) => {
                self.agents.blocked_only = !self.agents.blocked_only;
                self.agents.rebuild();
            }
            Key::Ctrl(CTRL_P) => self.agents.begin(Entry::Prompt, String::new()),
            // Prefilled with the name it has, so a rename is an edit rather than
            // a retype — and so the line says what is about to be replaced.
            Key::Ctrl(CTRL_R) => {
                let label = self.agents.selected_row().map(|row| row.name.clone());
                if let Some(label) = label {
                    self.agents.begin(Entry::Rename, label);
                }
            }
            Key::Ctrl(CTRL_X) => self.agents_kill(sink),
            Key::Ctrl(_) => {}
        }
        // The rebind *is* the move (X15): with a peek open, whatever the key did
        // to the selection is what the region should be showing by the time the
        // next frame is drawn. Nothing happens here — `App::settle_peek` is what
        // opens and closes — and a key that left the selection alone leaves the
        // intent alone with it.
        if self.agents.peek.is_some()
            && let Some(pane) = self.agents.selected_pane()
        {
            self.agents.peek = Some(pane);
        }
    }

    /// `Enter`: expand a collapsed run, or jump to the agent's pane.
    ///
    /// Jumping is a *presentation* move plus the one call that keeps the
    /// session's own focus from diverging, which is the shape every other focus
    /// change in this client already has (`super::super::actions`): the
    /// workspace this terminal shows moves locally and `workspace.switch` echoes
    /// it. There is no set-focus-to-this-pane op on the table — `pane.focus`
    /// speaks directions — so the pane half stays local, exactly as a numeric
    /// jump's does.
    fn agents_enter(&mut self, sink: &mut impl FnMut(InputEvent<'_>)) {
        match self.agents.visible.get(self.agents.at).copied() {
            Some(rows::Line::Collapsed { group, .. }) => {
                if !self.agents.expanded.remove(&group) {
                    self.agents.expanded.insert(group);
                }
                self.agents.rebuild();
            }
            Some(rows::Line::Agent(at)) => {
                let Some(row) = self.agents.rows.get(at) else {
                    return;
                };
                let (workspace, pane) = (row.workspace, row.pane);
                self.agents.open = false;
                self.agents.peek = None;
                self.model.focus_workspace(workspace);
                self.focus.insert(workspace, pane);
                sink(InputEvent::Call(Call::WorkspaceSwitch(
                    workspace::SwitchParams { workspace },
                )));
            }
            None => {}
        }
    }

    /// `ctrl+x`: arm on the first press, close the pane on the second.
    ///
    /// D15 adopts the twice-pattern from the prior art it audited and rejects a
    /// dialog to go with it: "press twice to confirm (no confirm dialog
    /// exists)". The armed pane is named in the header, so the second press is
    /// made with the answer on screen rather than from memory.
    fn agents_kill(&mut self, sink: &mut impl FnMut(InputEvent<'_>)) {
        let Some(pane) = self.agents.selected_pane() else {
            return;
        };
        if self.agents.armed == Some(pane) {
            self.agents.armed = None;
            sink(InputEvent::Call(Call::PaneClose(pane_proto::CloseParams {
                pane,
            })));
            return;
        }
        self.agents.armed = Some(pane);
    }

    /// One key while the entry line is open.
    fn agents_entry_key(&mut self, key: Key, sink: &mut impl FnMut(InputEvent<'_>)) {
        let Some((kind, text)) = self.agents.entry.as_mut() else {
            return;
        };
        match key {
            // The entry only, not the view: a cancelled prompt puts the board
            // back rather than taking the user off it.
            Key::Esc => self.agents.entry = None,
            Key::Back => {
                text.pop();
            }
            // A prompt is prose and a space is part of it, which is the one
            // place on this surface where the space bar types.
            Key::Space => text.push(' '),
            Key::Char(byte) => text.push(char::from(byte)),
            Key::Enter => {
                let (kind, text) = (*kind, std::mem::take(text));
                self.agents.entry = None;
                self.agents_submit(kind, text, sink);
            }
            // Nothing else reaches an entry: moving the selection under a prompt
            // being typed would send it to a different agent than the one named
            // on the line.
            Key::Up | Key::Down | Key::Ctrl(_) => {}
        }
    }

    /// Send what the entry line was taking.
    ///
    /// An empty prompt and an empty name are both refusals rather than calls:
    /// `agent.prompt ""` submits a bare newline to an agent and `pane.rename ""`
    /// would take a name away by accident.
    fn agents_submit(&mut self, kind: Entry, text: String, sink: &mut impl FnMut(InputEvent<'_>)) {
        let Some(pane) = self.agents.selected_pane() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        match kind {
            // No wait: D15's `ctrl+p` "prompts without attaching", and a view
            // that blocked on the agent's next transition would freeze the board
            // it was prompting from.
            Entry::Prompt => sink(InputEvent::Call(Call::AgentPrompt(
                agent_proto::PromptParams {
                    target: pane.into(),
                    text,
                    wait: PromptWait::None,
                    timeout_ms: None,
                },
            ))),
            Entry::Rename => sink(InputEvent::Call(Call::PaneRename(
                pane_proto::RenameParams { pane, label: text },
            ))),
        }
    }
}

impl AgentsUi {
    /// Open the entry line over the selected agent.
    pub(super) fn begin(&mut self, kind: Entry, text: String) {
        if self.selected_pane().is_some() {
            self.entry = Some((kind, text));
        }
    }

    /// Move the cursor by `delta` rows, stopping at either end.
    pub(super) fn step(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let last = self.visible.len() - 1;
        self.at = self.at.saturating_add_signed(delta).min(last);
        self.cursor = self
            .visible
            .get(self.at)
            .and_then(|line| line.cursor(&self.rows));
    }
}

/// Split one run of key bytes into the view's own vocabulary.
///
/// Two escape sequences are recognised and no more: `ESC [ A`/`B` and their
/// application-cursor-mode spelling `ESC O A`/`B`, which is what a terminal
/// sends for `↑`/`↓`. Any other sequence is consumed whole and dropped, so a key
/// this table has never heard of is *nothing* rather than an `Esc` that closes
/// the board followed by its own bytes typed into the filter.
pub(super) fn decode(bytes: &[u8], out: &mut Vec<Key>) {
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        i += 1;
        if byte != ESC {
            out.push(match byte {
                b'\r' | b'\n' => Key::Enter,
                0x08 | DEL => Key::Back,
                b' ' => Key::Space,
                0x00..=0x1f => Key::Ctrl(byte),
                0x21..=0x7e => Key::Char(byte),
                // Not a key this surface reads: a UTF-8 continuation byte, or
                // the leading byte of a character the filter has no use for.
                _ => continue,
            });
            continue;
        }
        match sequence(&bytes[i..]) {
            Sequence::Arrow(key, len) => {
                out.push(key);
                i += len;
            }
            Sequence::Other(len) => i += len,
            Sequence::None => out.push(Key::Esc),
        }
    }
}

/// What follows an `ESC`.
enum Sequence {
    /// An arrow this table reads, and how many bytes it took after the `ESC`.
    Arrow(Key, usize),
    /// A sequence this table does not read, and its length after the `ESC`.
    Other(usize),
    /// Nothing that makes a sequence: the `ESC` stands for itself.
    None,
}

/// Read the sequence introduced by an `ESC`, if `rest` holds one.
fn sequence(rest: &[u8]) -> Sequence {
    match rest.first() {
        // SS3: `ESC O A` is `↑` on a terminal in application-cursor-key mode.
        Some(b'O') => match rest.get(1) {
            Some(b'A') => Sequence::Arrow(Key::Up, 2),
            Some(b'B') => Sequence::Arrow(Key::Down, 2),
            Some(_) => Sequence::Other(2),
            None => Sequence::None,
        },
        Some(b'[') => {
            // Parameter and intermediate bytes, then one final byte: the CSI
            // grammar, read only far enough to know where the sequence ends.
            let mut at = 1;
            while rest.get(at).is_some_and(|&b| (0x20..0x40).contains(&b)) {
                at += 1;
            }
            match rest.get(at) {
                Some(b'A') if at == 1 => Sequence::Arrow(Key::Up, 2),
                Some(b'B') if at == 1 => Sequence::Arrow(Key::Down, 2),
                Some(_) => Sequence::Other(at + 1),
                None => Sequence::None,
            }
        }
        _ => Sequence::None,
    }
}
