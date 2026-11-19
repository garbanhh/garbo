//! Paleta de comandos con búsqueda difusa (Ctrl+Shift+P).

use crate::config::Theme;
use crate::keybindings::{Action, Keybindings};

#[derive(Clone, Debug)]
pub enum Item {
    Action(Action),
    Theme(&'static str),
}

pub struct Entry {
    pub label: String,
    pub hint: String,
    pub item: Item,
}

pub struct Palette {
    pub query: String,
    pub selected: usize,
    entries: Vec<Entry>,
    pub filtered: Vec<usize>,
    /// Rueda acumulada (fracciones de elemento).
    wheel: f32,
}

pub const MAX_VISIBLE: usize = 10;

impl Palette {
    pub fn new(kb: &Keybindings) -> Self {
        let mut entries: Vec<Entry> = Action::all()
            .into_iter()
            .filter(|(a, ..)| !matches!(a, Action::GotoTab(_) | Action::CommandPalette))
            .map(|(a, name, desc, _)| Entry {
                label: desc.to_string(),
                hint: kb.labels.get(name).cloned().unwrap_or_default(),
                item: Item::Action(a),
            })
            .collect();
        for &t in Theme::NAMES {
            entries.push(Entry { label: format!("Tema: {}", Theme::display_name(t)), hint: String::new(), item: Item::Theme(t) });
        }
        let mut p = Self { query: String::new(), selected: 0, entries, filtered: vec![], wheel: 0.0 };
        p.refilter();
        p
    }

    pub fn refilter(&mut self) {
        let mut scored: Vec<(i32, usize)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| fuzzy(&self.query, &e.label).map(|s| (s, i)))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        self.filtered = scored.into_iter().map(|(_, i)| i).collect();
        self.selected = 0;
    }

    pub fn entry(&self, idx_in_filtered: usize) -> Option<&Entry> {
        self.filtered.get(idx_in_filtered).map(|&i| &self.entries[i])
    }

    pub fn current(&self) -> Option<Item> {
        self.entry(self.selected).map(|e| e.item.clone())
    }

    /// Acumula la rueda y devuelve cuántos elementos enteros hay que avanzar (positivo = arriba).
    pub fn wheel(&mut self, steps: f32) -> i32 {
        self.wheel += steps;
        let n = self.wheel.trunc() as i32;
        self.wheel -= n as f32;
        n
    }

    pub fn move_sel(&mut self, delta: i32) {
        let n = self.filtered.len() as i32;
        if n > 0 {
            self.selected = (self.selected as i32 + delta).rem_euclid(n) as usize;
        }
    }

    /// Primer elemento visible para que la selección siempre quede en pantalla.
    pub fn scroll_start(&self) -> usize {
        self.selected.saturating_sub(MAX_VISIBLE - 1)
    }
}

/// Coincidencia por subsecuencia sin distinguir mayúsculas; premia letras consecutivas
/// e inicios de palabra.
fn fuzzy(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut ti = 0;
    let mut prev_match = false;
    for qc in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let mut found = false;
        while ti < text.len() {
            let tc = text[ti];
            ti += 1;
            if tc == qc {
                score += 1;
                if prev_match {
                    score += 3;
                }
                if ti == 1 || text[ti - 2] == ' ' {
                    score += 5;
                }
                prev_match = true;
                found = true;
                break;
            }
            prev_match = false;
        }
        if !found {
            return None;
        }
    }
    Some(score - text.len() as i32 / 8)
}
