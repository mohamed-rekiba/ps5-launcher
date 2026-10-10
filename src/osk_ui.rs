//! The on-screen keyboard: a docked QWERTY for controller users (docs/plans/ps5-launcher-os.md,
//! Phase 5; the prototype's pick A). It opens when a text field starts editing and the last input
//! came from a controller. The edited field is lifted above the keys, so it is never covered.

use crate::app::*;
use crate::audio::{self, Sound};
use crate::gamepad::Pad;
use crate::settings::SId;

/// The text field the keyboard edits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Library,
    SettingsFind,
    /// A text row of Settings: its index on the page, and what it is.
    Row(usize, SId),
}

/// The keys a field gets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layout {
    Text,
    /// Search fields. Library search also gets title suggestions.
    Search,
    /// Masked, with an eye key to show the text, and no suggestions.
    Password,
    /// Folders and files: `~`, `/`, `-`, `_` and `.` on the letters.
    Path,
}

pub fn layout_for(field: Field) -> Layout {
    match field {
        Field::Library | Field::SettingsFind => Layout::Search,
        Field::Row(_, SId::Rawg | SId::WifiPassword) => Layout::Password,
        Field::Row(_, SId::Dirs | SId::InstallDir | SId::DownloadDir | SId::Emulator | SId::ShadEmulator) => Layout::Path,
        Field::Row(..) => Layout::Text,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layer {
    Letters,
    /// Digits and the common symbols (`?123`).
    Symbols,
    /// The other symbols (`#+=` on the Symbols layer).
    More,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Char(char),
    Shift,
    Delete,
    /// `?123` on the letters, `ABC` on the symbols.
    Layer,
    Space,
    Done,
    /// Password fields: show or hide the text.
    Eye,
}

/// A key and where it sits in its row, in key widths from the left edge of the keys.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Cap {
    pub key: Key,
    pub x: f32,
    pub w: f32,
}

/// The four rows of keys.
pub fn rows(layout: Layout, layer: Layer) -> Vec<Vec<Cap>> {
    // A row of keys from `start`, each with its width.
    fn row(start: f32, keys: impl IntoIterator<Item = (Key, f32)>) -> Vec<Cap> {
        let mut x = start;
        keys.into_iter().map(|(key, w)| {
            let cap = Cap { key, x, w };
            x += w;
            cap
        }).collect()
    }
    let chars = |s: &str| s.chars().map(|c| (Key::Char(c), 1.0)).collect::<Vec<_>>();
    let (top, middle, bottom) = match layer {
        Layer::Letters => ("qwertyuiop", "asdfghjkl", "zxcvbnm"),
        Layer::Symbols => ("1234567890", "@#$%&-+()", "*\"':;!?"),
        Layer::More => ("~`|\\^{}[]=", "_<>/€£¥°•", ",©®¿¡«»"),
    };
    let last = match layout {
        Layout::Path => vec![(Key::Layer, 1.5), (Key::Char('~'), 1.0), (Key::Char('/'), 1.0), (Key::Char('-'), 1.0), (Key::Space, 2.0),
            (Key::Char('_'), 1.0), (Key::Char('.'), 1.0), (Key::Done, 1.5)],
        Layout::Password => vec![(Key::Layer, 1.5), (Key::Eye, 1.0), (Key::Space, 5.0), (Key::Char('.'), 1.0), (Key::Done, 1.5)],
        Layout::Text | Layout::Search => vec![(Key::Layer, 1.5), (Key::Char(','), 1.0), (Key::Space, 5.0), (Key::Char('.'), 1.0), (Key::Done, 1.5)],
    };
    vec![
        row(0.0, chars(top)),
        // The middle row is a half key in, like a real keyboard.
        row(0.5, chars(middle)),
        row(0.0, [(Key::Shift, 1.5)].into_iter().chain(chars(bottom)).chain([(Key::Delete, 1.5)])),
        row(0.0, last),
    ]
}

/// The key above (`down` false) or below a key: the one whose centre is closest, so moving
/// between rows of different lengths keeps the column. On a tie, down goes right and up goes
/// left, so up then down comes back to the same key. None past the top or bottom row.
pub fn step_row(rows: &[Vec<Cap>], r: usize, c: usize, down: bool) -> Option<(usize, usize)> {
    let to = if down { r + 1 } else { r.checked_sub(1)? };
    let target = rows.get(to)?;
    let centre = |k: &Cap| k.x + k.w / 2.0;
    let from = centre(rows.get(r)?.get(c)?);
    let mut best: Option<(usize, f32)> = None;
    for (i, k) in target.iter().enumerate() {
        let d = (centre(k) - from).abs();
        // Keys go left to right: on a tie, down keeps the later key and up the earlier one.
        let better = match best {
            None => true,
            Some((_, b)) => d < b - 1e-4 || (down && (d - b).abs() <= 1e-4),
        };
        if better {
            best = Some((i, d));
        }
    }
    best.map(|(i, _)| (to, i))
}

/// The accents for a letter, in its case. Empty for keys without accents.
pub fn accents(c: char) -> Vec<char> {
    let lower = c.to_lowercase().next().unwrap_or(c);
    let list = match lower {
        'a' => "àâäáãåæ",
        'c' => "ç",
        'e' => "éèêë",
        'i' => "îïíì",
        'n' => "ñ",
        'o' => "öôóòõøœ",
        'u' => "üùûú",
        'y' => "ÿý",
        _ => "",
    };
    if c == lower {
        list.chars().collect()
    } else {
        list.chars().map(|a| a.to_uppercase().next().unwrap_or(a)).collect()
    }
}

/// Up to 4 titles where a word starts with the typed text, those that start with it first.
/// Accents and punctuation do not count ("pokemon" finds "Pokémon").
pub fn suggest<'a>(titles: impl IntoIterator<Item = &'a str>, query: &str) -> Vec<String> {
    let q = crate::util::norm(query);
    let q = q.trim();
    if q.is_empty() {
        return Vec::new();
    }
    let word = format!(" {q}");
    let (mut first, mut later): (Vec<&str>, Vec<&str>) = (Vec::new(), Vec::new());
    for t in titles {
        if first.contains(&t) || later.contains(&t) {
            continue;
        }
        let n = crate::util::norm(t);
        if n.starts_with(q) {
            first.push(t);
        } else if n.contains(&word) {
            later.push(t);
        }
    }
    first.into_iter().chain(later).take(4).map(String::from).collect()
}

/// The edited text and the caret, both in characters (not bytes: accents are two bytes).
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Line {
    chars: Vec<char>,
    cursor: usize,
}

impl Line {
    /// The caret starts at the end.
    pub fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        Line { cursor: chars.len(), chars }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn insert(&mut self, s: &str) {
        for c in s.chars() {
            self.chars.insert(self.cursor, c);
            self.cursor += 1;
        }
    }

    /// Deletes the character before the caret.
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.chars.len());
    }

    /// The caret as a byte offset, for Slint's TextInput.
    pub fn byte_cursor(&self) -> usize {
        self.chars[..self.cursor].iter().map(|c| c.len_utf8()).sum()
    }

    /// The text before and after the caret, each character shown as a dot when `secret`.
    pub fn shown(&self, secret: bool) -> (String, String) {
        let show = |cs: &[char]| if secret { "•".repeat(cs.len()) } else { cs.iter().collect() };
        (show(&self.chars[..self.cursor]), show(&self.chars[self.cursor..]))
    }
}

/// What a press did, for the glue to act on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// Only the keyboard changed (focus, shift, layer).
    Moved,
    /// The text changed.
    Edited,
    /// Done: confirm the edit, as Enter does.
    Done,
    /// Close: cancel the edit, as Esc does.
    Close,
    /// A suggestion was chosen.
    Pick(usize),
}

/// The keyboard's state while it is open.
#[derive(Clone, Debug)]
pub struct Osk {
    pub field: Field,
    pub layout: Layout,
    pub line: Line,
    pub layer: Layer,
    /// The next letter is a capital.
    pub shift: bool,
    /// The focused key: row and column.
    pub r: usize,
    pub c: usize,
    /// Password fields: the text shows.
    pub eye: bool,
    /// The accent popup over the focused key: the accents and the focused one.
    pub accent: Option<(Vec<char>, usize)>,
    /// Title suggestions (Library search only), and the focused one when focus is on them.
    pub sugs: Vec<String>,
    pub sug: Option<usize>,
}

impl Osk {
    pub fn new(field: Field, text: &str) -> Self {
        Osk {
            field,
            layout: layout_for(field),
            line: Line::new(text),
            layer: Layer::Letters,
            shift: false,
            r: 0,
            c: 0,
            eye: false,
            accent: None,
            sugs: Vec::new(),
            sug: None,
        }
    }

    pub fn rows(&self) -> Vec<Vec<Cap>> {
        rows(self.layout, self.layer)
    }

    pub fn key(&self) -> Key {
        self.rows()[self.r][self.c].key
    }

    /// The character a key types now: a capital while shift is on.
    pub fn typed(&self, c: char) -> char {
        if self.shift && self.layer == Layer::Letters { c.to_uppercase().next().unwrap_or(c) } else { c }
    }

    /// New suggestions; focus leaves them when they are gone.
    pub fn set_sugs(&mut self, sugs: Vec<String>) {
        self.sug = match self.sug {
            Some(_) if sugs.is_empty() => None,
            Some(i) => Some(i.min(sugs.len() - 1)),
            None => None,
        };
        self.sugs = sugs;
    }

    /// A controller press.
    pub fn press(&mut self, p: Pad) -> Outcome {
        if let Some((list, i)) = &mut self.accent {
            match p {
                Pad::Left => *i = i.saturating_sub(1),
                Pad::Right => *i = (*i + 1).min(list.len() - 1),
                Pad::Confirm | Pad::ConfirmHold => {
                    let c = list[*i];
                    self.accent = None;
                    return self.type_char(c);
                }
                Pad::Back => self.accent = None,
                _ => {}
            }
            return Outcome::Moved;
        }
        match p {
            Pad::Square => {
                self.line.backspace();
                return Outcome::Edited;
            }
            Pad::Triangle => {
                self.line.insert(" ");
                return Outcome::Edited;
            }
            Pad::L1 => self.line.left(),
            Pad::R1 => self.line.right(),
            Pad::L2 => self.shift_key(),
            Pad::R2 => self.layer_key(),
            Pad::Options => return Outcome::Done,
            Pad::Back => return Outcome::Close,
            _ => return self.press_focus(p),
        }
        Outcome::Moved
    }

    /// Directions and Ⓐ, on the keys or on the suggestions.
    fn press_focus(&mut self, p: Pad) -> Outcome {
        if let Some(i) = self.sug {
            match p {
                Pad::Left => self.sug = Some(i.saturating_sub(1)),
                Pad::Right => self.sug = Some((i + 1).min(self.sugs.len().saturating_sub(1))),
                Pad::Down => self.sug = None,
                Pad::Confirm | Pad::ConfirmHold => return Outcome::Pick(i),
                _ => {}
            }
            return Outcome::Moved;
        }
        let rows = self.rows();
        let len = rows[self.r].len();
        match p {
            Pad::Left => self.c = if self.c == 0 { len - 1 } else { self.c - 1 },
            Pad::Right => self.c = if self.c + 1 == len { 0 } else { self.c + 1 },
            Pad::Up | Pad::Down => match step_row(&rows, self.r, self.c, p == Pad::Down) {
                Some((r, c)) => (self.r, self.c) = (r, c),
                None if p == Pad::Up && !self.sugs.is_empty() => self.sug = Some(0),
                None => {}
            },
            Pad::Confirm => return self.hit(),
            Pad::ConfirmHold => {
                if let Key::Char(c) = self.key() {
                    let list = accents(self.typed(c));
                    if !list.is_empty() {
                        self.accent = Some((list, 0));
                        return Outcome::Moved;
                    }
                }
                return self.hit();
            }
            _ => {}
        }
        Outcome::Moved
    }

    /// Press the focused key (Ⓐ, or a click on it).
    pub fn hit(&mut self) -> Outcome {
        match self.key() {
            Key::Char(c) => return self.type_char(self.typed(c)),
            Key::Shift => self.shift_key(),
            Key::Delete => {
                self.line.backspace();
                return Outcome::Edited;
            }
            Key::Layer => self.layer_key(),
            Key::Space => {
                self.line.insert(" ");
                return Outcome::Edited;
            }
            Key::Done => return Outcome::Done,
            Key::Eye => self.eye = !self.eye,
        }
        Outcome::Moved
    }

    fn type_char(&mut self, c: char) -> Outcome {
        self.line.insert(&c.to_string());
        if self.layer == Layer::Letters {
            self.shift = false;
        }
        Outcome::Edited
    }

    /// Shift on the letters; on the symbols, the other symbols and back.
    fn shift_key(&mut self) {
        match self.layer {
            Layer::Letters => self.shift = !self.shift,
            Layer::Symbols => self.set_layer(Layer::More),
            Layer::More => self.set_layer(Layer::Symbols),
        }
    }

    /// `?123` and `ABC`.
    fn layer_key(&mut self) {
        self.set_layer(if self.layer == Layer::Letters { Layer::Symbols } else { Layer::Letters });
    }

    fn set_layer(&mut self, layer: Layer) {
        self.layer = layer;
        self.c = self.c.min(self.rows()[self.r].len() - 1);
    }
}

// ====================================================================== glue

/// The keyboard on screen, and the keys last sent to the UI (they are sent again only when they
/// change, so the focus moves without rebuilding them).
#[derive(Default)]
pub struct OskUi {
    pub osk: Option<Osk>,
    keys: Vec<crate::OskKey>,
}

impl OskUi {
    pub fn open(&self) -> bool {
        self.osk.is_some()
    }
}

/// What a key shows: its label, or an icon.
fn face(osk: &Osk, key: Key) -> (String, &'static str) {
    match key {
        Key::Char(c) => (osk.typed(c).to_string(), ""),
        Key::Shift => match osk.layer {
            Layer::Letters => (String::new(), "shift"),
            Layer::Symbols => ("#+=".into(), ""),
            Layer::More => ("123".into(), ""),
        },
        Key::Delete => (String::new(), "backspace"),
        Key::Layer => (if osk.layer == Layer::Letters { "?123" } else { "ABC" }.into(), ""),
        Key::Space => ("space".into(), ""),
        Key::Done => ("Done".into(), ""),
        Key::Eye => (String::new(), if osk.eye { "eye" } else { "eyeoff" }),
    }
}

/// A typed character from a physical keyboard: not a control key, and not one of Slint's
/// special keys (arrows, function keys: the private use area).
fn printable(text: &str) -> bool {
    let mut chars = text.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if !c.is_control() && !('\u{e000}'..='\u{f8ff}').contains(&c))
}

impl App {
    /// The keyboard takes `field`, which starts with `text`. The field's own editing state (the
    /// search, the Settings row) is already set; the keyboard only replaces the typing.
    pub fn osk_open(&mut self, field: Field, text: &str) {
        crate::gamepad::set_confirm_hold(true);
        self.osk = OskUi { osk: Some(Osk::new(field, text)), keys: Vec::new() };
        if field == Field::Library {
            self.osk_suggest();
        }
        let ui = self.ui();
        ui.set_osk_open(true);
        ui.invoke_focus_root();
        self.osk_push();
    }

    /// Close the keyboard if it edits a field `which` accepts. Only the keyboard: the fields' own
    /// stops (`stop_search_edit`, `settings_find_stop`, `finish_edit`) call this, so anything that
    /// ends editing also closes it.
    pub fn osk_close_if(&mut self, which: impl Fn(Field) -> bool) {
        if self.osk.osk.as_ref().is_some_and(|o| which(o.field)) {
            self.osk = OskUi::default();
            crate::gamepad::set_confirm_hold(false);
            self.ui().set_osk_open(false);
        }
    }

    /// A controller press while the keyboard is open. It gets every press but the PS button.
    pub fn osk_pad(&mut self, p: Pad) {
        let Some(osk) = self.osk.osk.as_mut() else { return };
        let out = osk.press(p);
        self.osk_outcome(out);
    }

    /// A click on key `i` (an index into the keys as sent to the UI).
    pub fn osk_click(&mut self, i: usize) {
        let Some(osk) = self.osk.osk.as_mut() else { return };
        let mut n = i;
        for (r, row) in osk.rows().iter().enumerate() {
            if n < row.len() {
                (osk.r, osk.c, osk.sug, osk.accent) = (r, n, None, None);
                let out = osk.hit();
                return self.osk_outcome(out);
            }
            n -= row.len();
        }
    }

    pub fn osk_pick(&mut self, i: usize) {
        self.osk_outcome(Outcome::Pick(i));
    }

    fn osk_outcome(&mut self, out: Outcome) {
        match out {
            Outcome::Moved => {
                audio::play(Sound::Move);
                self.osk_push();
            }
            Outcome::Edited => {
                self.osk_apply();
                self.osk_push();
            }
            Outcome::Done => self.osk_finish(true),
            Outcome::Close => self.osk_finish(false),
            Outcome::Pick(i) => {
                let Some(osk) = self.osk.osk.as_mut() else { return };
                let Some(title) = osk.sugs.get(i).cloned() else { return };
                osk.line = Line::new(&title);
                self.osk_apply();
                self.osk_finish(true);
            }
        }
    }

    /// The text changed: search fields filter as you type; a Settings row waits for Done.
    fn osk_apply(&mut self) {
        let Some(osk) = &self.osk.osk else { return };
        let (field, text) = (osk.field, osk.line.text());
        let ui = self.ui();
        match field {
            Field::Library => {
                ui.set_query(text.clone().into());
                self.on_search(text);
                self.osk_suggest();
            }
            Field::SettingsFind => {
                ui.set_settings_query(text.clone().into());
                self.settings_find_edited(text);
            }
            Field::Row(..) => ui.set_edit_text(text.into()),
        }
    }

    /// Titles from the catalog for the Library search.
    fn osk_suggest(&mut self) {
        let Some(osk) = &self.osk.osk else { return };
        let sugs = suggest(self.games.iter().map(|g| g.name.as_str()), &osk.line.text());
        if let Some(osk) = self.osk.osk.as_mut() {
            osk.set_sugs(sugs);
        }
    }

    /// Done confirms the edit, as Enter does; Close cancels it, as Esc does.
    pub fn osk_finish(&mut self, done: bool) {
        let Some(osk) = &self.osk.osk else { return };
        let (field, text) = (osk.field, osk.line.text());
        if !done {
            audio::play(Sound::Back);
        }
        match (field, done) {
            (Field::Library, true) => self.search_done(),
            (Field::Library, false) => self.stop_search_edit(),
            (Field::SettingsFind, true) => self.settings_find_done(),
            (Field::SettingsFind, false) => self.settings_find_stop(),
            (Field::Row(..), true) => {
                audio::play(Sound::Select);
                self.finish_edit(Some(text));
            }
            (Field::Row(..), false) => self.finish_edit(None),
        }
        self.osk_close_if(|_| true);
    }

    /// A physical key while the keyboard is open. Enter and Esc work as Done and Close. Any
    /// other key closes the keyboard and the field takes the typing, with the key typed in it.
    /// Returns false when the key is left for the app's own shortcuts (Ctrl and Alt).
    pub fn osk_key(&mut self, text: &str, ctrl: bool, alt: bool) -> bool {
        use slint::platform::Key as K;
        let Some(osk) = self.osk.osk.as_mut() else { return false };
        let k = |key: K| slint::SharedString::from(key) == text;
        if !ctrl && !alt {
            if k(K::Escape) {
                self.osk_finish(false);
                return true;
            }
            if k(K::Return) || text == "\r" {
                self.osk_finish(true);
                return true;
            }
            if k(K::Backspace) {
                osk.line.backspace();
                self.osk_apply();
            } else if printable(text) {
                osk.line.insert(text);
                self.osk_apply();
            }
        }
        self.osk_hand_over();
        !(ctrl || alt)
    }

    /// The field takes the keyboard where the on-screen keyboard left it, caret included.
    fn osk_hand_over(&mut self) {
        let Some(osk) = &self.osk.osk else { return };
        let (field, at) = (osk.field, osk.line.byte_cursor() as i32);
        let ui = self.ui();
        ui.set_edit_caret(at);
        self.osk_close_if(|_| true);
        // The Settings fields are made again now that the keyboard is closed, and take the
        // keyboard with the caret at `edit-caret` themselves.
        if field == Field::Library {
            ui.invoke_focus_search_at(at);
        }
    }

    fn osk_push(&mut self) {
        let Some(osk) = &self.osk.osk else { return };
        let rows = osk.rows();
        let keys: Vec<crate::OskKey> = rows.iter().enumerate().flat_map(|(r, row)| {
            row.iter().map(move |cap| {
                let (label, icon) = face(osk, cap.key);
                crate::OskKey {
                    label: label.into(),
                    icon: icon.into(),
                    row: r as i32,
                    x: cap.x,
                    w: cap.w,
                    special: !matches!(cap.key, Key::Char(_)),
                    lit: cap.key == Key::Shift && osk.shift && osk.layer == Layer::Letters,
                }
            })
        }).collect();
        let focus = if osk.sug.is_some() { -1 } else { rows[..osk.r].iter().map(Vec::len).sum::<usize>() as i32 + osk.c as i32 };
        let secret = osk.layout == Layout::Password;
        let (before, after) = osk.line.shown(secret && !osk.eye);
        let (label, placeholder) = match osk.field {
            Field::Library => ("Search your library".to_string(), "Game title"),
            Field::SettingsFind => ("Search settings".to_string(), "Setting name"),
            Field::Row(i, id) => {
                let label = self.settings_rows.get(i).map(|r| r.label.to_string()).unwrap_or_default();
                let placeholder = match id {
                    SId::Rawg => "API key",
                    SId::WifiPassword => "Wi-Fi password",
                    _ => "",
                };
                (label, placeholder)
            }
        };
        let field = crate::OskField {
            label: label.into(),
            placeholder: placeholder.into(),
            before: before.into(),
            after: after.into(),
            secret,
            eye: osk.eye,
            search: osk.layout == Layout::Search,
            mono: matches!(osk.layout, Layout::Path | Layout::Password),
            shift: match osk.layer {
                Layer::Letters if osk.shift => "Shift on",
                Layer::Letters => "Shift",
                Layer::Symbols => "#+=",
                Layer::More => "123",
            }.into(),
            layer: if osk.layer == Layer::Letters { "?123" } else { "ABC" }.into(),
        };
        let accents: Vec<slint::SharedString> = osk.accent.as_ref().map(|(list, _)| list.iter().map(|c| c.to_string().into()).collect()).unwrap_or_default();
        let accent = osk.accent.as_ref().map(|(_, i)| *i as i32).unwrap_or(0);
        let sugs: Vec<slint::SharedString> = osk.sugs.iter().map(|s| s.into()).collect();
        let sug = osk.sug.map(|i| i as i32).unwrap_or(-1);
        let ui = self.ui();
        if keys != self.osk.keys {
            ui.set_osk_keys(model(keys.clone()));
            self.osk.keys = keys;
        }
        ui.set_osk_focus(focus);
        ui.set_osk_field(field);
        ui.set_osk_accents(model(accents));
        ui.set_osk_accent(accent);
        ui.set_osk_sugs(model(sugs));
        ui.set_osk_sug(sug);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(rows: &[Vec<Cap>], key: Key) -> (usize, usize) {
        for (r, row) in rows.iter().enumerate() {
            if let Some(c) = row.iter().position(|cap| cap.key == key) {
                return (r, c);
            }
        }
        panic!("{key:?} is not on the keyboard");
    }

    fn key_at(rows: &[Vec<Cap>], rc: Option<(usize, usize)>) -> Option<Key> {
        rc.map(|(r, c)| rows[r][c].key)
    }

    #[test]
    fn each_field_gets_its_layout() {
        assert_eq!(layout_for(Field::Library), Layout::Search);
        assert_eq!(layout_for(Field::SettingsFind), Layout::Search);
        assert_eq!(layout_for(Field::Row(3, SId::Rawg)), Layout::Password);
        assert_eq!(layout_for(Field::Row(4, SId::WifiPassword)), Layout::Password);
        for id in [SId::Dirs, SId::InstallDir, SId::DownloadDir, SId::Emulator, SId::ShadEmulator] {
            assert_eq!(layout_for(Field::Row(0, id)), Layout::Path, "{id:?}");
        }
        assert_eq!(layout_for(Field::Row(0, SId::Extra)), Layout::Text);
    }

    #[test]
    fn every_row_is_ten_keys_wide_and_the_qwerty_is_in_order() {
        for layout in [Layout::Text, Layout::Search, Layout::Password, Layout::Path] {
            for layer in [Layer::Letters, Layer::Symbols, Layer::More] {
                let rows = rows(layout, layer);
                assert_eq!(rows.len(), 4);
                for (r, row) in rows.iter().enumerate() {
                    let end = row.last().map(|k| k.x + k.w).unwrap();
                    let start = if r == 1 { 0.5 } else { 0.0 };
                    assert_eq!((row[0].x, end), (start, if r == 1 { 9.5 } else { 10.0 }), "{layout:?} {layer:?} row {r}");
                    for pair in row.windows(2) {
                        assert_eq!(pair[0].x + pair[0].w, pair[1].x, "keys touch: {layout:?} {layer:?} row {r}");
                    }
                }
            }
        }
        let letters = rows(Layout::Text, Layer::Letters);
        let chars = |r: usize| letters[r].iter().filter_map(|k| if let Key::Char(c) = k.key { Some(c) } else { None }).collect::<String>();
        assert_eq!((chars(0), chars(1), chars(2)), ("qwertyuiop".into(), "asdfghjkl".into(), "zxcvbnm".into()));
        assert_eq!(letters[2][0].key, Key::Shift);
        assert_eq!(letters[2][8].key, Key::Delete);
    }

    #[test]
    fn the_bottom_row_follows_the_field() {
        let bottom = |layout| rows(layout, Layer::Letters)[3].iter().map(|k| k.key).collect::<Vec<_>>();
        assert_eq!(bottom(Layout::Search), [Key::Layer, Key::Char(','), Key::Space, Key::Char('.'), Key::Done]);
        assert_eq!(bottom(Layout::Password), [Key::Layer, Key::Eye, Key::Space, Key::Char('.'), Key::Done]);
        assert_eq!(bottom(Layout::Path), [Key::Layer, Key::Char('~'), Key::Char('/'), Key::Char('-'), Key::Space, Key::Char('_'), Key::Char('.'), Key::Done]);
    }

    #[test]
    fn every_printable_ascii_character_can_be_typed() {
        for layout in [Layout::Text, Layout::Password, Layout::Path] {
            let mut typable = String::from(" ");
            for layer in [Layer::Letters, Layer::Symbols, Layer::More] {
                for k in rows(layout, layer).iter().flatten() {
                    if let Key::Char(c) = k.key {
                        typable.push(c);
                        typable.extend(c.to_uppercase());
                    }
                }
            }
            for c in (0x20u8..0x7f).map(char::from) {
                assert!(typable.contains(c), "{c:?} missing on {layout:?}");
            }
        }
    }

    #[test]
    fn moving_between_rows_keeps_the_closest_column() {
        let rows = rows(Layout::Search, Layer::Letters);
        let from = |key| at(&rows, key);
        let go = |key, down| key_at(&rows, { let (r, c) = from(key); step_row(&rows, r, c, down) });
        assert_eq!(go(Key::Char('q'), true), Some(Key::Char('a')));
        assert_eq!(go(Key::Char('p'), true), Some(Key::Char('l')));
        assert_eq!(go(Key::Char('a'), true), Some(Key::Shift), "a sits over the wide shift key");
        assert_eq!(go(Key::Char('s'), true), Some(Key::Char('z')));
        assert_eq!(go(Key::Char('l'), true), Some(Key::Delete));
        assert_eq!(go(Key::Shift, true), Some(Key::Layer));
        assert_eq!(go(Key::Char('v'), true), Some(Key::Space));
        assert_eq!(go(Key::Space, false), Some(Key::Char('v')));
        assert_eq!(go(Key::Done, false), Some(Key::Delete));
        assert_eq!(go(Key::Char('q'), false), None, "past the top");
        assert_eq!(go(Key::Space, true), None, "past the bottom");
    }

    #[test]
    fn down_then_up_comes_back_to_the_same_letter() {
        let rows = rows(Layout::Search, Layer::Letters);
        // p has no letter below it on the right, so it is the one exception: p, l, o.
        for c in 0..9 {
            let below = step_row(&rows, 0, c, true).unwrap();
            assert_eq!(step_row(&rows, below.0, below.1, false), Some((0, c)), "{:?}", rows[0][c].key);
        }
        // t sits over f and g; down goes right.
        let (r, c) = at(&rows, Key::Char('t'));
        assert_eq!(key_at(&rows, step_row(&rows, r, c, true)), Some(Key::Char('g')));
    }

    #[test]
    fn line_edits_work_on_characters() {
        let mut l = Line::new("café");
        assert_eq!(l.byte_cursor(), 5, "é is two bytes");
        l.backspace();
        assert_eq!(l.text(), "caf");
        l.insert("è");
        l.left();
        l.left();
        l.insert("x");
        assert_eq!(l.text(), "caxfè");
        assert_eq!(l.byte_cursor(), 3);
        assert_eq!(l.shown(false), ("cax".into(), "fè".into()));
        assert_eq!(l.shown(true), ("•••".into(), "••".into()));
        l.right();
        l.right();
        l.right();
        assert_eq!(l.byte_cursor(), "caxfè".len(), "the caret stops at the end");
    }

    #[test]
    fn backspace_and_left_stop_at_the_start() {
        let mut l = Line::new("ab");
        l.left();
        l.left();
        l.left();
        l.backspace();
        assert_eq!((l.text().as_str(), l.byte_cursor()), ("ab", 0));
        let mut empty = Line::new("");
        empty.backspace();
        assert_eq!(empty.text(), "");
    }

    #[test]
    fn letters_have_accents_in_their_case() {
        assert_eq!(accents('e'), ['é', 'è', 'ê', 'ë']);
        assert!(accents('a').starts_with(&['à', 'â', 'ä']));
        assert!(accents('u').starts_with(&['ü', 'ù', 'û']));
        assert!(accents('o').starts_with(&['ö', 'ô']));
        assert_eq!(accents('c'), ['ç']);
        assert_eq!(accents('n'), ['ñ']);
        assert_eq!(accents('E'), ['É', 'È', 'Ê', 'Ë']);
        assert!(accents('q').is_empty());
        assert!(accents('1').is_empty());
    }

    #[test]
    fn suggestions_match_word_starts_and_ignore_accents() {
        let titles = ["Astro Bot", "Astro's Playroom", "Ratchet & Clank: Rift Apart", "Demon's Souls", "Pokémon Legends", "Gastro Ball"];
        assert_eq!(suggest(titles, "astro"), ["Astro Bot", "Astro's Playroom"], "a word start, not inside a word");
        assert_eq!(suggest(titles, "rift"), ["Ratchet & Clank: Rift Apart"]);
        assert_eq!(suggest(titles, "pokemon"), ["Pokémon Legends"]);
        assert_eq!(suggest(titles, "  "), Vec::<String>::new());
        assert_eq!(suggest(titles, "zzz"), Vec::<String>::new());
    }

    #[test]
    fn suggestions_put_title_starts_first_skip_repeats_and_stop_at_four() {
        let titles = ["The Last of Us", "Last Labyrinth", "Last Labyrinth", "Lasting", "Last Stop", "Last Day", "Last One"];
        assert_eq!(suggest(titles, "last"), ["Last Labyrinth", "Lasting", "Last Stop", "Last Day"]);
        assert_eq!(suggest(["The Last of Us", "Last Stop"], "LAST"), ["Last Stop", "The Last of Us"]);
    }

    fn osk(field: Field, text: &str) -> Osk {
        Osk::new(field, text)
    }

    fn focus(o: &mut Osk, key: Key) {
        (o.r, o.c) = at(&o.rows(), key);
    }

    #[test]
    fn a_new_keyboard_starts_on_q_with_the_caret_at_the_end() {
        let o = osk(Field::Library, "ast");
        assert_eq!((o.key(), o.layer, o.shift, o.line.byte_cursor()), (Key::Char('q'), Layer::Letters, false, 3));
    }

    #[test]
    fn the_face_buttons_and_shoulders_edit() {
        let mut o = osk(Field::Library, "ab");
        assert_eq!(o.press(Pad::Square), Outcome::Edited);
        assert_eq!(o.press(Pad::Triangle), Outcome::Edited);
        assert_eq!(o.line.text(), "a ");
        assert_eq!(o.press(Pad::L1), Outcome::Moved);
        assert_eq!(o.press(Pad::Confirm), Outcome::Edited, "q at the caret");
        assert_eq!(o.line.text(), "aq ");
        assert_eq!(o.press(Pad::R1), Outcome::Moved);
        assert_eq!(o.line.byte_cursor(), 3);
        assert_eq!(o.press(Pad::Options), Outcome::Done);
        assert_eq!(o.press(Pad::Back), Outcome::Close);
    }

    #[test]
    fn shift_capitalises_one_letter() {
        let mut o = osk(Field::Row(0, SId::Extra), "");
        o.press(Pad::L2);
        assert!(o.shift);
        o.press(Pad::Confirm);
        o.press(Pad::Confirm);
        assert_eq!(o.line.text(), "Qq");
        assert!(!o.shift);
        focus(&mut o, Key::Shift);
        o.press(Pad::Confirm);
        assert!(o.shift, "the shift key does the same");
    }

    #[test]
    fn r2_switches_to_symbols_and_keeps_the_focus_on_the_keys() {
        let mut o = osk(Field::Row(0, SId::Extra), "");
        focus(&mut o, Key::Char('p'));
        assert_eq!(o.press(Pad::R2), Outcome::Moved);
        assert_eq!((o.layer, o.key()), (Layer::Symbols, Key::Char('0')));
        o.press(Pad::Confirm);
        // L2 on the symbols shows the other symbols, and back.
        o.press(Pad::L2);
        assert_eq!(o.layer, Layer::More);
        o.press(Pad::L2);
        assert_eq!(o.layer, Layer::Symbols);
        o.press(Pad::R2);
        assert_eq!(o.layer, Layer::Letters);
        // The layer key does the same as R2.
        focus(&mut o, Key::Layer);
        o.press(Pad::Confirm);
        assert_eq!(o.layer, Layer::Symbols);
        assert_eq!(o.line.text(), "0");
    }

    #[test]
    fn directions_wrap_in_a_row_and_move_between_rows() {
        let mut o = osk(Field::Library, "");
        o.press(Pad::Left);
        assert_eq!(o.key(), Key::Char('p'));
        o.press(Pad::Right);
        assert_eq!(o.key(), Key::Char('q'));
        o.press(Pad::Down);
        assert_eq!(o.key(), Key::Char('a'));
        o.press(Pad::Up);
        assert_eq!(o.key(), Key::Char('q'));
        assert_eq!(o.press(Pad::Up), Outcome::Moved, "no suggestions: nothing above");
        assert_eq!((o.key(), o.sug), (Key::Char('q'), None));
    }

    #[test]
    fn holding_a_on_a_letter_opens_its_accents() {
        let mut o = osk(Field::Library, "caf");
        focus(&mut o, Key::Char('e'));
        assert_eq!(o.press(Pad::ConfirmHold), Outcome::Moved);
        assert_eq!(o.accent, Some((vec!['é', 'è', 'ê', 'ë'], 0)));
        o.press(Pad::Right);
        o.press(Pad::Right);
        o.press(Pad::Up); // ignored while the popup is open
        assert_eq!(o.press(Pad::Confirm), Outcome::Edited);
        assert_eq!((o.line.text().as_str(), o.accent.is_none()), ("cafê", true));
        // Back closes only the popup.
        o.press(Pad::ConfirmHold);
        assert_eq!(o.press(Pad::Back), Outcome::Moved);
        assert!(o.accent.is_none());
        assert_eq!(o.line.text(), "cafê");
    }

    #[test]
    fn accents_follow_shift_and_a_hold_without_accents_types_the_key() {
        let mut o = osk(Field::Library, "");
        focus(&mut o, Key::Char('e'));
        o.press(Pad::L2);
        o.press(Pad::ConfirmHold);
        o.press(Pad::Confirm);
        assert_eq!((o.line.text().as_str(), o.shift), ("É", false));
        focus(&mut o, Key::Char('w'));
        assert_eq!(o.press(Pad::ConfirmHold), Outcome::Edited);
        assert_eq!(o.line.text(), "Éw");
    }

    #[test]
    fn suggestions_sit_above_the_keys() {
        let mut o = osk(Field::Library, "ast");
        o.set_sugs(vec!["Astro Bot".into(), "Astro's Playroom".into()]);
        o.press(Pad::Up);
        assert_eq!(o.sug, Some(0));
        o.press(Pad::Right);
        o.press(Pad::Right);
        assert_eq!(o.sug, Some(1), "stops at the last one");
        assert_eq!(o.press(Pad::Confirm), Outcome::Pick(1));
        o.press(Pad::Down);
        assert_eq!((o.sug, o.r), (None, 0));
        o.press(Pad::Up);
        o.set_sugs(Vec::new());
        assert_eq!(o.sug, None, "focus leaves suggestions that are gone");
    }

    #[test]
    fn the_eye_key_shows_a_password() {
        let mut o = osk(Field::Row(5, SId::Rawg), "key");
        assert!(!o.eye);
        focus(&mut o, Key::Eye);
        assert_eq!(o.press(Pad::Confirm), Outcome::Moved);
        assert!(o.eye);
    }

    #[test]
    fn the_done_and_delete_keys() {
        let mut o = osk(Field::Library, "ab");
        focus(&mut o, Key::Delete);
        assert_eq!(o.press(Pad::Confirm), Outcome::Edited);
        assert_eq!(o.line.text(), "a");
        focus(&mut o, Key::Done);
        assert_eq!(o.press(Pad::Confirm), Outcome::Done);
    }
}
