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
enum Action {
    Added(Vec<StrokeId>),
    Erased(Vec<(StrokeId, StrokeData)>),
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
        self.redo.clear();
        self.undo.push(Action::Added(vec![id]));
        self.local_op(OpKind::StrokeAdd { id, data })
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
            if self.erased.contains(id) {
                continue;
            }
            let Some(e) = self.strokes.get(id) else {
                continue;
            };
            group.push((*id, e.data.clone()));
            ops.push(self.local_op(OpKind::StrokeErase { id: *id }));
        }
        if !group.is_empty() {
            self.redo.clear();
            match self.undo.last_mut() {
                Some(Action::Erased(prev)) if continue_group => prev.extend(group),
                _ => self.undo.push(Action::Erased(group)),
            }
        }
        ops
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
        let Some(action) = self.undo.pop() else {
            return Vec::new();
        };
        let (ops, inverse) = self.invert(action);
        self.redo.push(inverse);
        ops
    }

    pub fn redo(&mut self) -> Vec<Op> {
        let Some(action) = self.redo.pop() else {
            return Vec::new();
        };
        let (ops, inverse) = self.invert(action);
        self.undo.push(inverse);
        ops
    }

    fn invert(&mut self, action: Action) -> (Vec<Op>, Action) {
        match action {
            Action::Added(ids) => {
                let mut group = Vec::new();
                let mut ops = Vec::new();
                for id in ids {
                    if let Some(e) = self.strokes.get(&id) {
                        if !self.erased.contains(&id) {
                            group.push((id, e.data.clone()));
                            ops.push(self.local_op(OpKind::StrokeErase { id }));
                        }
                    }
                }
                (ops, Action::Erased(group))
            }
            Action::Erased(list) => {
                let mut ids = Vec::new();
                let mut ops = Vec::new();
                for (old, data) in list {
                    let id = self.fresh_id();
                    ids.push(id);
                    ops.push(self.local_op(OpKind::StrokeAdd { id, data }));
                    // Kreska odrodzila sie pod nowym id. Starsze wpisy historii
                    // (np. jej pierwotne dodanie) musza od teraz wskazywac na nowe,
                    // inaczej kolejne undo trafialoby w nagrobek i nic nie robilo.
                    self.remap(old, id);
                }
                (ops, Action::Added(ids))
            }
        }
    }

    fn remap(&mut self, old: StrokeId, new: StrokeId) {
        for action in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            match action {
                Action::Added(ids) => {
                    for id in ids.iter_mut() {
                        if *id == old {
                            *id = new;
                        }
                    }
                }
                Action::Erased(list) => {
                    for (id, _) in list.iter_mut() {
                        if *id == old {
                            *id = new;
                        }
                    }
                }
            }
        }
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
