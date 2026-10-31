use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, HashSet};

use spectre_proto::{AuthorId, Op, OpKind, StrokeData, StrokeId};

/// Prostokat otaczajacy w przestrzeni canvasu, z uwzglednieniem szerokosci kreski.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bbox {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

impl Bbox {
    pub fn of(data: &StrokeData) -> Self {
        let mut b = Bbox {
            min_x: f32::INFINITY,
            min_y: f32::INFINITY,
            max_x: f32::NEG_INFINITY,
            max_y: f32::NEG_INFINITY,
        };
        for s in &data.samples {
            b.min_x = b.min_x.min(s.x);
            b.min_y = b.min_y.min(s.y);
            b.max_x = b.max_x.max(s.x);
            b.max_y = b.max_y.max(s.y);
        }
        if data.samples.is_empty() {
            return Bbox {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 0.0,
                max_y: 0.0,
            };
        }
        // Kreska ma grubosc - bez tego kafel na granicy obcialby jej krawedz.
        b.expand(data.base_width * 0.5 + 1.0)
    }

    pub fn expand(self, r: f32) -> Self {
        Bbox {
            min_x: self.min_x - r,
            min_y: self.min_y - r,
            max_x: self.max_x + r,
            max_y: self.max_y + r,
        }
    }

    pub fn intersects(&self, o: &Bbox) -> bool {
        self.min_x <= o.max_x
            && o.min_x <= self.max_x
            && self.min_y <= o.max_y
            && o.min_y <= self.max_y
    }
}

struct Entry {
    data: StrokeData,
    bbox: Bbox,
    key: (u64, AuthorId),
}

/// Jedna akcja uzytkownika w sensie undo. Grupa, bo jedno pociagniecie gumka
/// potrafi wymazac wiele kresek i cofa sie je razem.
///
/// Same identyfikatory, bez danych kresek: dokument trzyma dane takze kresek
/// wymazanych (nagrobek je tylko ukrywa), wiec cofniecie wymazania bierze je
/// stamtad. Dzieki temu historia jest mala i da sie ja zapisac na dysku
/// (`history_bytes`) - cofanie dziala takze po ponownym uruchomieniu.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Added(Vec<StrokeId>),
    Erased(Vec<StrokeId>),
    /// Podmiana: stare kreski wymazane, nowe (przeksztalcone) dodane. Dla
    /// uzytkownika to jeden ruch zaznaczenia, wiec cofa sie w calosci.
    Replaced {
        erased: Vec<StrokeId>,
        added: Vec<StrokeId>,
    },
}

/// Tyle akcji trzyma historia; starsze odpadaja. Akcja to kilkanascie bajtow
/// na kreske, wiec nawet pelna historia to pojedyncze megabajty.
pub const HISTORY_MAX: usize = 100_000;

/// Historia cofania wyjeta z dokumentu (`take_history`) - do przeniesienia
/// na nowo wczytany dokument tej samej notatki.
pub struct History {
    undo: Vec<Action>,
    redo: Vec<Action>,
}

pub struct Document {
    author: AuthorId,
    clock: u64,
    next_seq: u64,

    /// Kolejnosc renderowania: (lamport, author) -> id. Deterministyczna u wszystkich.
    order: BTreeMap<(u64, AuthorId), StrokeId>,
    /// Indeks przestrzenny po Y (Z9: rolka pionowa): pas `BAND` jednostek -> zywe
    /// kreski, ktorych bbox go przecina. Zapytanie o widoczny prostokat czyta
    /// kilka pasow zamiast skanowac wszystkie kreski (przy 100 000 kresek skan
    /// kosztowal ~5 ms na klatke - wiecej niz samo rysowanie).
    bands: BTreeMap<i32, Vec<StrokeId>>,
    /// Obwiednia zywej tresci; `None` w cache = do przeliczenia (po wymazaniu).
    content: Cell<Option<Option<Bbox>>>,
    strokes: HashMap<StrokeId, Entry>,
    /// Nagrobki. Moga wyprzedzac `StrokeAdd` (operacje z sieci przychodza w dowolnej kolejnosci).
    erased: HashSet<StrokeId>,
    /// Metadane: klucz -> ((lamport, author), wartosc). LWW.
    meta: HashMap<String, ((u64, AuthorId), String)>,
    /// Idempotencja: zbior zastosowanych (author, lamport).
    applied: HashSet<(AuthorId, u64)>,

    undo: Vec<Action>,
    redo: Vec<Action>,
    /// Grupa wymazania otwarta: kolejne `erase_strokes_continuing` dopisuja sie
    /// do niej. Zamyka ja kazda inna akcja i `close_group` (poczatek gestu gumki).
    erase_open: bool,
}

impl Document {
    pub fn new(author: AuthorId) -> Self {
        Self {
            author,
            clock: 0,
            next_seq: 1,
            order: BTreeMap::new(),
            bands: BTreeMap::new(),
            content: Cell::new(Some(None)),
            strokes: HashMap::new(),
            erased: HashSet::new(),
            meta: HashMap::new(),
            applied: HashSet::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            erase_open: false,
        }
    }

    pub fn author(&self) -> AuthorId {
        self.author
    }

    pub fn lamport(&self) -> u64 {
        self.clock
    }

    /// Zastosowanie operacji - lokalnej, z dysku albo z sieci; bez roznicy.
    /// Zwraca `false`, gdy operacja byla juz zastosowana.
    pub fn apply(&mut self, op: &Op) -> bool {
        if !self.applied.insert((op.author, op.lamport)) {
            return false;
        }
        self.clock = self.clock.max(op.lamport);
        match &op.kind {
            OpKind::StrokeAdd { id, data } => {
                if id.author == self.author {
                    self.next_seq = self.next_seq.max(id.seq + 1);
                }
                let key = op.key();
                self.strokes.insert(
                    *id,
                    Entry {
                        data: data.clone(),
                        bbox: Bbox::of(data),
                        key,
                    },
                );
                if !self.erased.contains(id) {
                    self.order.insert(key, *id);
                    self.index_insert(*id);
                }
            }
            OpKind::StrokeErase { id } => {
                if self.erased.insert(*id) {
                    if let Some(e) = self.strokes.get(id) {
                        let key = e.key;
                        self.order.remove(&key);
                        self.index_remove(*id);
                    }
                }
            }
            OpKind::Meta { key, value } => {
                let stamp = op.key();
                let newer = match self.meta.get(key) {
                    Some((old, _)) => stamp > *old,
                    None => true,
                };
                if newer {
                    self.meta.insert(key.clone(), (stamp, value.clone()));
                }
            }
        }
        true
    }

    fn local_op(&mut self, kind: OpKind) -> Op {
        self.clock += 1;
        let op = Op {
            author: self.author,
            lamport: self.clock,
            kind,
        };
        self.apply(&op);
        op
    }

    fn fresh_id(&mut self) -> StrokeId {
        let id = StrokeId {
            author: self.author,
            seq: self.next_seq,
        };
        self.next_seq += 1;
        id
    }

    /// Nowa kreska. Zwrocona operacja idzie do pliku i do peerow.
    pub fn add_stroke(&mut self, data: StrokeData) -> Op {
        let id = self.fresh_id();
        self.push_action(Action::Added(vec![id]));
        self.local_op(OpKind::StrokeAdd { id, data })
    }

    /// Kilka kresek jako **jedna** akcja historii (wklejenie schowka): jedno
    /// cofniecie zdejmuje calosc, nie kreske po kresce.
    pub fn add_strokes(&mut self, data: &[StrokeData]) -> (Vec<Op>, Vec<StrokeId>) {
        let mut ops = Vec::new();
        let mut ids = Vec::new();
        for d in data {
            let id = self.fresh_id();
            ids.push(id);
            ops.push(self.local_op(OpKind::StrokeAdd {
                id,
                data: d.clone(),
            }));
        }
        if !ids.is_empty() {
            self.push_action(Action::Added(ids.clone()));
        }
        (ops, ids)
    }

    /// Wymazanie zbioru kresek jako jednej akcji. Nieznane/juz wymazane sa pomijane.
    pub fn erase_strokes(&mut self, ids: &[StrokeId]) -> Vec<Op> {
        self.erase_impl(ids, false)
    }

    /// Jak `erase_strokes`, ale dolacza do poprzedniej grupy wymazania, jesli
    /// ostatnia akcja nia byla. Jedno pociagniecie gumka to wiele wywolan
    /// (po jednym na paczke probek), a cofac chcemy je razem.
    pub fn erase_strokes_continuing(&mut self, ids: &[StrokeId]) -> Vec<Op> {
        self.erase_impl(ids, true)
    }

    fn erase_impl(&mut self, ids: &[StrokeId], continue_group: bool) -> Vec<Op> {
        let mut ops = Vec::new();
        let mut group = Vec::new();
        for id in ids {
            if self.erased.contains(id) || !self.strokes.contains_key(id) {
                continue;
            }
            group.push(*id);
            ops.push(self.local_op(OpKind::StrokeErase { id: *id }));
        }
        if !group.is_empty() {
            match self.undo.last_mut() {
                // Dopisanie tylko w obrebie tego samego gestu gumki. Wczesniej
                // wystarczalo, ze ostatnia akcja byla wymazaniem - dwa osobne
                // pociagniecia gumka cofaly sie razem jednym `Ctrl+Z`.
                Some(Action::Erased(prev)) if continue_group && self.erase_open => {
                    self.redo.clear();
                    prev.extend(group);
                }
                _ => self.push_action(Action::Erased(group)),
            }
            self.erase_open = continue_group;
        }
        ops
    }

    /// Podmiana kresek: stare znikaja, nowe wchodza na ich miejsce jako
    /// **jedna** akcja w historii. Tak dziala przesuniecie, skalowanie
    /// i obrot zaznaczenia - kreska jest niezmienna, a nagrobek nieodwracalny
    /// (ADR 0004), wiec "edycja" to zawsze wymazanie plus dodanie.
    ///
    /// Zwraca operacje do zapisania/wyslania i **nowe** identyfikatory
    /// (zaznaczenie musi od teraz wskazywac na nie).
    pub fn replace_strokes(
        &mut self,
        ids: &[StrokeId],
        data: &[StrokeData],
    ) -> (Vec<Op>, Vec<StrokeId>) {
        let mut ops = Vec::new();
        let mut erased = Vec::new();
        let mut added = Vec::new();
        for (old, new) in ids.iter().zip(data) {
            if !self.strokes.contains_key(old) || self.erased.contains(old) {
                continue;
            }
            erased.push(*old);
            ops.push(self.local_op(OpKind::StrokeErase { id: *old }));
            let id = self.fresh_id();
            added.push(id);
            ops.push(self.local_op(OpKind::StrokeAdd {
                id,
                data: new.clone(),
            }));
        }
        if !added.is_empty() {
            self.push_action(Action::Replaced {
                erased,
                added: added.clone(),
            });
        }
        (ops, added)
    }

    pub fn set_meta(&mut self, key: &str, value: &str) -> Op {
        self.local_op(OpKind::Meta {
            key: key.to_string(),
            value: value.to_string(),
        })
    }

    pub fn meta(&self, key: &str) -> Option<&str> {
        self.meta.get(key).map(|(_, v)| v.as_str())
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Cofniecie ostatniej akcji. Zwraca operacje do zapisania/wyslania.
    ///
    /// Cofniecie wymazania to **nowa** kreska z nowym id - nagrobek jest
    /// nieodwracalny, bo tylko wtedy CRDT pozostaje zbiorem rosnacym (ADR 0004).
    pub fn undo(&mut self) -> Vec<Op> {
        self.erase_open = false;
        let Some(action) = self.undo.pop() else {
            return Vec::new();
        };
        let (ops, inverse) = self.invert(action);
        self.redo.push(inverse);
        ops
    }

    pub fn redo(&mut self) -> Vec<Op> {
        self.erase_open = false;
        let Some(action) = self.redo.pop() else {
            return Vec::new();
        };
        let (ops, inverse) = self.invert(action);
        self.undo.push(inverse);
        ops
    }

    /// Nowa akcja uzytkownika: konczy grupe gumki, kasuje redo, a historia
    /// dluzsza niz `HISTORY_MAX` traci najstarsze wpisy.
    fn push_action(&mut self, action: Action) {
        self.erase_open = false;
        self.redo.clear();
        self.undo.push(action);
        if self.undo.len() > HISTORY_MAX + HISTORY_MAX / 10 {
            let cut = self.undo.len() - HISTORY_MAX;
            self.undo.drain(..cut);
        }
    }

    /// Poczatek nowego gestu gumki: nastepne wymazanie zaczyna nowa akcje.
    pub fn close_group(&mut self) {
        self.erase_open = false;
    }

    /// Zywe kreski z listy - wymazywane przy odwracaniu dodania.
    fn erase_live(&mut self, ids: Vec<StrokeId>, ops: &mut Vec<Op>) -> Vec<StrokeId> {
        let mut done = Vec::new();
        for id in ids {
            if self.strokes.contains_key(&id) && !self.erased.contains(&id) {
                done.push(id);
                ops.push(self.local_op(OpKind::StrokeErase { id }));
            }
        }
        done
    }

    /// Kreski z listy odrodzone pod nowymi id (dane z dokumentu - nagrobek ich
    /// nie usuwa). Nieznane (np. historia z innej wersji pliku) sa pomijane.
    fn revive(&mut self, ids: Vec<StrokeId>, ops: &mut Vec<Op>) -> Vec<StrokeId> {
        let mut done = Vec::new();
        for old in ids {
            let Some(data) = self.strokes.get(&old).map(|e| e.data.clone()) else {
                continue;
            };
            let id = self.fresh_id();
            done.push(id);
            ops.push(self.local_op(OpKind::StrokeAdd { id, data }));
            // Kreska odrodzila sie pod nowym id. Starsze wpisy historii
            // (np. jej pierwotne dodanie) musza od teraz wskazywac na nowe,
            // inaczej kolejne undo trafialoby w nagrobek i nic nie robilo.
            self.remap(old, id);
        }
        done
    }

    fn invert(&mut self, action: Action) -> (Vec<Op>, Action) {
        let mut ops = Vec::new();
        let inverse = match action {
            Action::Added(ids) => Action::Erased(self.erase_live(ids, &mut ops)),
            Action::Erased(ids) => Action::Added(self.revive(ids, &mut ops)),
            // Cofniecie podmiany: nowe kreski znikaja, stare wracaja (pod
            // nowymi id). Odwrotnoscia jest ta sama akcja z zamieniona
            // kolejnoscia, wiec redo odtwarza ruch zaznaczenia.
            Action::Replaced { erased, added } => {
                let now_erased = self.erase_live(added, &mut ops);
                let now_added = self.revive(erased, &mut ops);
                Action::Replaced {
                    erased: now_erased,
                    added: now_added,
                }
            }
        };
        (ops, inverse)
    }

    fn remap(&mut self, old: StrokeId, new: StrokeId) {
        for action in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            let lists: [&mut Vec<StrokeId>; 2] = match action {
                Action::Added(ids) | Action::Erased(ids) => [ids, &mut Vec::new()],
                Action::Replaced { erased, added } => [erased, added],
            };
            for list in lists {
                for id in list.iter_mut() {
                    if *id == old {
                        *id = new;
                    }
                }
            }
        }
    }

    /// Historia wyjeta z dokumentu - po ponownym wczytaniu tej samej notatki
    /// (merge z gita) wraca przez `set_history`, zamiast przepadac.
    pub fn take_history(&mut self) -> History {
        self.erase_open = false;
        History {
            undo: std::mem::take(&mut self.undo),
            redo: std::mem::take(&mut self.redo),
        }
    }

    pub fn set_history(&mut self, h: History) {
        self.erase_open = false;
        self.undo = h.undo;
        self.redo = h.redo;
    }

    /// Historia cofania jako bajty - do pliku lokalnego (nie do gita: to stan
    /// tego urzadzenia, nie notatki). Wpisy to same identyfikatory kresek.
    pub fn history_bytes(&self) -> Vec<u8> {
        use spectre_proto::varint::put_u64;
        let mut out = Vec::with_capacity(16 + self.undo.len() * 12);
        out.extend_from_slice(HISTORY_MAGIC);
        let mut last_author = None;
        for stack in [&self.undo, &self.redo] {
            put_u64(&mut out, stack.len() as u64);
            for a in stack {
                let (tag, lists): (u8, [&[StrokeId]; 2]) = match a {
                    Action::Added(ids) => (1, [ids, &[]]),
                    Action::Erased(ids) => (2, [ids, &[]]),
                    Action::Replaced { erased, added } => (3, [erased, added]),
                };
                out.push(tag);
                for list in &lists[..if tag == 3 { 2 } else { 1 }] {
                    put_u64(&mut out, list.len() as u64);
                    for id in *list {
                        // Autor zwykle ten sam co wyzej: 0 = "jak poprzednio",
                        // 1 + skrot = inny (skrot ma 8 bajtow, varint nawet 10).
                        if last_author == Some(id.author) {
                            out.push(0);
                        } else {
                            out.push(1);
                            out.extend_from_slice(&id.author.0.to_le_bytes());
                            last_author = Some(id.author);
                        }
                        put_u64(&mut out, id.seq);
                    }
                }
            }
        }
        out
    }

    /// Odczyt `history_bytes`. Uszkodzony albo obcy plik = pusta historia
    /// (cofanie zaczyna sie od zera, notatka jest nietknieta). Wpisy o kreskach,
    /// ktorych dokument nie zna, przy cofaniu sa po prostu pomijane.
    pub fn load_history(&mut self, bytes: &[u8]) -> bool {
        match decode_history(bytes) {
            Some((undo, redo)) => {
                self.set_history(History { undo, redo });
                true
            }
            None => false,
        }
    }

    /// Dlugosci stosow cofania i ponawiania (HUD, testy).
    pub fn history_len(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }

    pub fn get(&self, id: StrokeId) -> Option<&StrokeData> {
        self.strokes.get(&id).map(|e| &e.data)
    }

    pub fn is_live(&self, id: StrokeId) -> bool {
        self.strokes.contains_key(&id) && !self.erased.contains(&id)
    }

    pub fn live_count(&self) -> usize {
        self.order.len()
    }

    /// Zywe kreski w kolejnosci renderowania.
    pub fn visible(&self) -> impl Iterator<Item = (StrokeId, &StrokeData, &Bbox)> + '_ {
        self.order.values().map(move |id| {
            let e = &self.strokes[id];
            (*id, &e.data, &e.bbox)
        })
    }

    /// Zywe kreski przecinajace prostokat, w kolejnosci renderowania.
    /// Z indeksu pasow; wynik jest maly (to, co widac), wiec sortowanie po
    /// kluczu kolejnosci jest tanie.
    pub fn visible_in(&self, rect: Bbox) -> impl Iterator<Item = (StrokeId, &StrokeData, &Bbox)> {
        let mut hits: Vec<(&Entry, StrokeId)> = Vec::new();
        for ids in self
            .bands
            .range(band_of(rect.min_y)..=band_of(rect.max_y))
            .map(|(_, v)| v)
        {
            for id in ids {
                let e = &self.strokes[id];
                if e.bbox.intersects(&rect) {
                    hits.push((e, *id));
                }
            }
        }
        // Kreska lezaca w kilku pasach trafila tu kilka razy.
        hits.sort_by_key(|(e, _)| e.key);
        hits.dedup_by_key(|(e, _)| e.key);
        hits.into_iter().map(|(e, id)| (id, &e.data, &e.bbox))
    }

    /// Dolna krawedz tresci - do ograniczenia przewijania.
    pub fn content_bottom(&self) -> f32 {
        self.content_bbox().map(|b| b.max_y).unwrap_or(0.0).max(0.0)
    }

    /// Obwiednia calej zywej tresci; `None` dla pustej notatki. Cache: rosnie
    /// przy dodaniu, przeliczana leniwie po wymazaniu.
    pub fn content_bbox(&self) -> Option<Bbox> {
        if let Some(c) = self.content.get() {
            return c;
        }
        let c = self.visible().map(|(_, _, b)| *b).reduce(union);
        self.content.set(Some(c));
        c
    }

    fn index_insert(&mut self, id: StrokeId) {
        let bbox = self.strokes[&id].bbox;
        for band in band_of(bbox.min_y)..=band_of(bbox.max_y) {
            self.bands.entry(band).or_default().push(id);
        }
        if let Some(c) = self.content.get() {
            self.content.set(Some(Some(match c {
                Some(old) => union(old, bbox),
                None => bbox,
            })));
        }
    }

    fn index_remove(&mut self, id: StrokeId) {
        let bbox = self.strokes[&id].bbox;
        for band in band_of(bbox.min_y)..=band_of(bbox.max_y) {
            if let Some(v) = self.bands.get_mut(&band) {
                v.retain(|x| *x != id);
                if v.is_empty() {
                    self.bands.remove(&band);
                }
            }
        }
        self.content.set(None);
    }
}

/// Naglowek pliku historii (`Document::history_bytes`), wersja 1.
const HISTORY_MAGIC: &[u8; 4] = b"SNH1";

/// Odwrotnosc `history_bytes`; `None` przy jakiejkolwiek niezgodnosci.
fn decode_history(bytes: &[u8]) -> Option<(Vec<Action>, Vec<Action>)> {
    let rest = bytes.strip_prefix(HISTORY_MAGIC)?;
    let mut r = spectre_proto::varint::Reader::new(rest);
    let mut author = None;
    let mut stacks = [Vec::new(), Vec::new()];
    for stack in &mut stacks {
        let n = r.usize().ok()?;
        // Rozmiar z pliku nie moze zarezerwowac wiecej niz plik pomiesci.
        stack.reserve(n.min(r.remaining()));
        for _ in 0..n {
            let tag = r.u8().ok()?;
            let mut lists = [Vec::new(), Vec::new()];
            let count = match tag {
                1 | 2 => 1,
                3 => 2,
                _ => return None,
            };
            for list in lists.iter_mut().take(count) {
                let len = r.usize().ok()?;
                if len > r.remaining() {
                    return None;
                }
                for _ in 0..len {
                    match r.u8().ok()? {
                        0 => {}
                        1 => author = Some(AuthorId(r.u64_le().ok()?)),
                        _ => return None,
                    }
                    let seq = r.u64().ok()?;
                    list.push(StrokeId {
                        author: author?,
                        seq,
                    });
                }
            }
            let [a, b] = lists;
            stack.push(match tag {
                1 => Action::Added(a),
                2 => Action::Erased(a),
                _ => Action::Replaced {
                    erased: a,
                    added: b,
                },
            });
        }
    }
    if !r.is_empty() {
        return None;
    }
    let [undo, redo] = stacks;
    Some((undo, redo))
}

/// Wysokosc pasa indeksu w jednostkach canvasu (ok. 1/3 ekranu przy zoomie 1).
const BAND: f32 = 512.0;

#[inline]
fn band_of(y: f32) -> i32 {
    (y / BAND).floor() as i32
}

fn union(a: Bbox, b: Bbox) -> Bbox {
    Bbox {
        min_x: a.min_x.min(b.min_x),
        min_y: a.min_y.min(b.min_y),
        max_x: a.max_x.max(b.max_x),
        max_y: a.max_y.max(b.max_y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use spectre_proto::{Rgba, Sample};

    fn stroke(n: usize, seed: f32) -> StrokeData {
        StrokeData {
            tool: 0,
            color: Rgba::rgb(200, 200, 200),
            base_width: 3.0,
            samples: (0..n)
                .map(|i| Sample {
                    x: seed + i as f32,
                    y: seed * 2.0,
                    pressure: 0.5,
                    ..Default::default()
                })
                .collect(),
        }
    }

    fn snapshot(d: &Document) -> Vec<(StrokeId, usize)> {
        d.visible()
            .map(|(id, s, _)| (id, s.samples.len()))
            .collect()
    }

    /// Scenariusz: kilku autorow, kazdy tworzy i wymazuje swoje i cudze kreski.
    /// Generujemy operacje tak, jak powstalyby lokalnie, a potem sprawdzamy,
    /// czy dowolna permutacja daje ten sam wynik.
    fn ops_strategy() -> impl Strategy<Value = Vec<Op>> {
        prop::collection::vec((0u8..3, 0u8..4, 1usize..6), 1..40).prop_map(|script| {
            let authors = [AuthorId(1), AuthorId(2), AuthorId(3)];
            let mut docs: Vec<Document> = authors.iter().map(|a| Document::new(*a)).collect();
            let mut all_ops: Vec<Op> = Vec::new();
            for (who, what, n) in script {
                let who = who as usize;
                // Kazdy autor widzi wszystko, co powstalo dotad (najgorszy przypadek
                // dla przemiennosci to i tak losowa kolejnosc ponizej).
                for op in &all_ops {
                    docs[who].apply(op);
                }
                let new_ops = match what {
                    0 | 1 => vec![docs[who].add_stroke(stroke(n, n as f32))],
                    2 => {
                        let victim = docs[who].visible().map(|(id, _, _)| id).next();
                        match victim {
                            Some(id) => docs[who].erase_strokes(&[id]),
                            None => vec![],
                        }
                    }
                    _ => docs[who].undo(),
                };
                all_ops.extend(new_ops);
            }
            all_ops
        })
    }

    proptest! {
        #[test]
        fn przemiennosc(ops in ops_strategy(), seed in any::<u64>()) {
            let mut a = Document::new(AuthorId(99));
            for op in &ops {
                a.apply(op);
            }
            // Deterministyczna permutacja z ziarna.
            let mut shuffled = ops.clone();
            let mut s = seed | 1;
            for i in (1..shuffled.len()).rev() {
                s ^= s << 13; s ^= s >> 7; s ^= s << 17;
                let j = (s % (i as u64 + 1)) as usize;
                shuffled.swap(i, j);
            }
            let mut b = Document::new(AuthorId(99));
            for op in &shuffled {
                b.apply(op);
            }
            prop_assert_eq!(snapshot(&a), snapshot(&b));
        }

        #[test]
        fn idempotencja(ops in ops_strategy()) {
            let mut a = Document::new(AuthorId(99));
            for op in &ops {
                a.apply(op);
            }
            let before = snapshot(&a);
            for op in &ops {
                prop_assert!(!a.apply(op), "powtorka nie moze byc uznana za nowa");
            }
            prop_assert_eq!(snapshot(&a), before);
        }
    }

    #[test]
    fn undo_redo_symetria() {
        let mut d = Document::new(AuthorId(1));
        d.add_stroke(stroke(3, 1.0));
        let id2 = match d.add_stroke(stroke(4, 2.0)).kind {
            OpKind::StrokeAdd { id, .. } => id,
            _ => unreachable!(),
        };
        assert_eq!(d.live_count(), 2);

        d.erase_strokes(&[id2]);
        assert_eq!(d.live_count(), 1);

        let ops = d.undo();
        assert_eq!(ops.len(), 1, "cofniecie wymazania = jedna nowa kreska");
        assert_eq!(d.live_count(), 2);
        assert!(!d.is_live(id2), "stary id zostaje nagrobkiem");

        d.redo();
        assert_eq!(d.live_count(), 1);
        d.undo();
        d.undo();
        assert_eq!(d.live_count(), 1);
        d.undo();
        assert_eq!(d.live_count(), 0);
        assert!(d.undo().is_empty());
    }

    fn add_id(d: &mut Document, n: usize, seed: f32) -> StrokeId {
        match d.add_stroke(stroke(n, seed)).kind {
            OpKind::StrokeAdd { id, .. } => id,
            _ => unreachable!(),
        }
    }

    /// Jeden gest gumki (wiele paczek probek) to jedna akcja; dwa gesty pod
    /// rzad - dwie. Wczesniej drugi doklejal sie do pierwszego i `Ctrl+Z`
    /// przywracal oba naraz.
    #[test]
    fn osobne_gesty_gumki_cofaja_sie_osobno() {
        let mut d = Document::new(AuthorId(1));
        let ids: Vec<StrokeId> = (0..4).map(|i| add_id(&mut d, 3, i as f32)).collect();
        // Gest 1: dwie paczki probek.
        d.close_group();
        d.erase_strokes_continuing(&[ids[0]]);
        d.erase_strokes_continuing(&[ids[1]]);
        // Gest 2.
        d.close_group();
        d.erase_strokes_continuing(&[ids[2]]);
        assert_eq!(d.live_count(), 1);
        d.undo();
        assert_eq!(d.live_count(), 2, "cofa tylko drugi gest");
        d.undo();
        assert_eq!(d.live_count(), 4, "pierwszy gest w calosci");
        // Wymazanie po cofnieciu tez zaczyna nowa akcje.
        d.erase_strokes_continuing(&[ids[3]]);
        d.undo();
        assert_eq!(d.live_count(), 4);
    }

    /// Historia przezywa "restart": dokument wczytany od nowa z tych samych
    /// operacji plus historia z bajtow cofa i ponawia dokladnie tak samo,
    /// lacznie z cofnieciem wymazania (dane kreski z nagrobka).
    #[test]
    fn historia_przezywa_ponowne_wczytanie() {
        let mut d = Document::new(AuthorId(1));
        let a = add_id(&mut d, 3, 1.0);
        let b = add_id(&mut d, 4, 2.0);
        let _ = add_id(&mut d, 5, 3.0);
        let mut ops: Vec<Op> = Vec::new();
        // Odtworzenie op-logu: dokument od nowa i te same operacje.
        let replay = |d: &Document| {
            let mut all: Vec<Op> = Vec::new();
            for (id, e) in &d.strokes {
                all.push(Op {
                    author: id.author,
                    lamport: e.key.0,
                    kind: OpKind::StrokeAdd {
                        id: *id,
                        data: e.data.clone(),
                    },
                });
            }
            all
        };
        ops.extend(d.erase_strokes(&[a]));
        ops.extend(d.replace_strokes(&[b], &[stroke(4, 9.0)]).0);
        d.undo(); // cofa podmiane
        assert_eq!(d.history_len(), (3 + 1, 1));
        let bytes = d.history_bytes();

        // "Restart": nowy dokument z kreskami (takze wymazanymi) i nagrobkami.
        let mut d2 = Document::new(AuthorId(1));
        for op in replay(&d) {
            d2.apply(&op);
        }
        for id in &d.erased {
            d2.apply(&Op {
                author: AuthorId(1),
                lamport: 10_000 + id.seq,
                kind: OpKind::StrokeErase { id: *id },
            });
        }
        assert_eq!(d2.live_count(), d.live_count());
        assert!(d2.load_history(&bytes));
        assert_eq!(d2.history_len(), d.history_len());
        // Te same kroki daja ten sam stan w obu.
        for step in 0..6 {
            let (x, y) = if step == 0 {
                (d.redo(), d2.redo())
            } else {
                (d.undo(), d2.undo())
            };
            assert_eq!(x.len(), y.len(), "krok {step}");
            assert_eq!(d.live_count(), d2.live_count(), "krok {step}");
        }
        assert_eq!(d2.live_count(), 0, "cofniete do pustej notatki");
        // Uszkodzony plik: brak historii, dokument nietkniety.
        let mut d3 = Document::new(AuthorId(1));
        assert!(!d3.load_history(&bytes[..bytes.len() - 1]));
        assert!(!d3.load_history(b"XXXX"));
        assert!(!d3.can_undo());
    }

    /// Przesuniecie zaznaczenia: podmiana kresek to jedna akcja historii -
    /// jedno cofniecie wraca do stanu sprzed ruchu, jedno ponowienie odtwarza go.
    #[test]
    fn podmiana_kresek_cofa_sie_jako_jeden_ruch() {
        let mut d = Document::new(AuthorId(1));
        let mut ids = Vec::new();
        for i in 0..3 {
            let op = d.add_stroke(stroke(3, i as f32 + 1.0));
            if let OpKind::StrokeAdd { id, .. } = op.kind {
                ids.push(id);
            }
        }
        let moved: Vec<StrokeData> = ids
            .iter()
            .map(|id| {
                let mut s = d.get(*id).unwrap().clone();
                for p in &mut s.samples {
                    p.x += 100.0;
                }
                s
            })
            .collect();
        let (ops, new_ids) = d.replace_strokes(&ids, &moved);
        assert_eq!(ops.len(), 6, "3 nagrobki + 3 nowe kreski");
        assert_eq!(new_ids.len(), 3);
        assert_eq!(d.live_count(), 3);
        assert!(ids.iter().all(|id| !d.is_live(*id)));
        let xs = |d: &Document| -> Vec<f32> {
            let mut v: Vec<f32> = d.visible().map(|(_, s, _)| s.samples[0].x).collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v
        };
        assert_eq!(xs(&d), vec![101.0, 102.0, 103.0]);

        d.undo();
        assert_eq!(d.live_count(), 3);
        assert_eq!(
            xs(&d),
            vec![1.0, 2.0, 3.0],
            "jedno cofniecie cofa caly ruch"
        );
        d.redo();
        assert_eq!(xs(&d), vec![101.0, 102.0, 103.0]);
        // Kolejne cofniecia schodza do stanu sprzed kresek - historia sprzed
        // podmiany musi wskazywac na kreski, ktore odzyly pod nowymi id.
        d.undo();
        d.undo();
        d.undo();
        d.undo();
        assert_eq!(d.live_count(), 0);
    }

    #[test]
    fn nagrobek_wyprzedzajacy_dodanie() {
        // Z sieci moze przyjsc Erase przed Add - wynik ma byc taki sam.
        let mut src = Document::new(AuthorId(1));
        let add = src.add_stroke(stroke(2, 0.0));
        let id = match add.kind {
            OpKind::StrokeAdd { id, .. } => id,
            _ => unreachable!(),
        };
        let erase = src.erase_strokes(&[id]).remove(0);

        let mut d = Document::new(AuthorId(2));
        d.apply(&erase);
        d.apply(&add);
        assert_eq!(d.live_count(), 0);
    }

    #[test]
    fn licznik_seq_odtwarza_sie_z_wczytanych_operacji() {
        let mut d1 = Document::new(AuthorId(1));
        let ops = vec![d1.add_stroke(stroke(1, 0.0)), d1.add_stroke(stroke(1, 1.0))];
        // "Restart": nowy dokument tego samego autora wczytuje swoje operacje.
        let mut d2 = Document::new(AuthorId(1));
        for op in &ops {
            d2.apply(op);
        }
        let next = d2.add_stroke(stroke(1, 2.0));
        match next.kind {
            OpKind::StrokeAdd { id, .. } => assert_eq!(id.seq, 3),
            _ => unreachable!(),
        }
        assert!(next.lamport > ops[1].lamport);
    }

    #[test]
    fn indeks_pasow_zgodny_ze_skanem() {
        let mut d = Document::new(AuthorId(1));
        let mut ids = Vec::new();
        for i in 0..400 {
            // Rozne wysokosci, niektore kreski dlugie w pionie (kilka pasow).
            let seed = (i as f32 * 37.0) % 3000.0;
            let mut s = stroke(5, seed);
            if i % 7 == 0 {
                s.samples.push(Sample {
                    x: seed,
                    y: seed * 2.0 + 1500.0,
                    pressure: 0.5,
                    ..Default::default()
                });
            }
            let op = d.add_stroke(s);
            if let OpKind::StrokeAdd { id, .. } = op.kind {
                ids.push(id);
            }
        }
        for id in ids.iter().step_by(3) {
            d.erase_strokes(&[*id]);
        }
        for (y0, y1) in [
            (0.0, 700.0),
            (1000.0, 1100.0),
            (-50.0, 6500.0),
            (5900.0, 5901.0),
        ] {
            let rect = Bbox {
                min_x: -1e9,
                min_y: y0,
                max_x: 1e9,
                max_y: y1,
            };
            let fast: Vec<StrokeId> = d.visible_in(rect).map(|(id, _, _)| id).collect();
            let slow: Vec<StrokeId> = d
                .visible()
                .filter(|(_, _, b)| b.intersects(&rect))
                .map(|(id, _, _)| id)
                .collect();
            assert_eq!(fast, slow, "pas {y0}..{y1}");
        }
        let slow_bottom = d.visible().map(|(_, _, b)| b.max_y).fold(0.0, f32::max);
        assert_eq!(d.content_bottom(), slow_bottom);
        let _ = d.undo();
        let slow_bottom = d.visible().map(|(_, _, b)| b.max_y).fold(0.0, f32::max);
        assert_eq!(d.content_bottom(), slow_bottom);
    }
}
