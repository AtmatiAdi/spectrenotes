//! "Send feedback": okno nad canvasem z polem tekstowym, zalacznikami (plik
//! z dysku, zrzut aplikacji) i przyciskiem, ktory zaklada issue w publicznym
//! repozytorium wydan (`spectre_update::default_repo()`) tokenem zalogowanego
//! uzytkownika. Zalaczniki laduja w publicznym repo `spectrenotes-feedback`
//! na koncie uzytkownika (Contents API) - issue w cudzym repo nie przyjmie
//! pliku, a obrazek z surowego adresu GitHub renderuje w tresci.
//!
//! Bez logowania: przegladarka z formularzem nowego issue i wpisana trescia
//! (bez zalacznikow).
//!
//! Wysylka trwa sekundy; w tym czasie okno pokazuje pasek postepu ze
//! scenariuszem komentarzy (easter egg) - pasek potrafi sie cofnac, gdy
//! komentarz mowi, ze cos nie wyszlo. Scenariusz gra do konca nawet gdy
//! GitHub odpowiedzial szybciej; prawdziwy blad przerywa go od razu.

use std::time::Instant;

use spectre_proto::Rgba;
use spectre_render::{UiFont, UiPrim};

use crate::github::{self, Issue};
use crate::ui::{Rect, ACCENT, BG, FG, FG_DIM, HOT, LINE};

/// Repo (na koncie uzytkownika) na zalaczniki.
pub const FILES_REPO: &str = "spectrenotes-feedback";
/// Limit tresci i lacznej wagi zalacznikow.
pub const TEXT_MAX: usize = 5000;
pub const ATTACH_MAX: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Attachment {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct Draft {
    pub text: String,
    pub attachments: Vec<Attachment>,
}

impl Draft {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.attachments.is_empty()
    }

    pub fn attached_bytes(&self) -> usize {
        self.attachments.iter().map(|a| a.bytes.len()).sum()
    }
}

/// Co doklejamy pod trescia, zeby autor aplikacji nie musial dopytywac.
#[derive(Debug, Clone, Default)]
pub struct Info {
    pub version: String,
    pub os: String,
    pub gpu: String,
    pub window: String,
    pub sync: String,
}

impl Info {
    fn markdown(&self) -> String {
        format!(
            "---\n- SpectreNotes {}\n- {}\n- GPU: {}\n- window: {}\n- sync: {}\n",
            self.version, self.os, self.gpu, self.window, self.sync
        )
    }
}

fn title_of(draft: &Draft) -> String {
    let first = draft
        .text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut t: String = first.chars().take(80).collect();
    if first.chars().count() > 80 {
        t.push('…');
    }
    if t.is_empty() {
        t = if draft.attachments.is_empty() {
            "Feedback".to_string()
        } else {
            "Feedback (attachment)".to_string()
        };
    }
    t
}

fn is_image(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp"]
        .iter()
        .any(|e| lower.ends_with(e))
}

/// Nazwa pliku bezpieczna w sciezce repo.
fn safe_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let s = s.trim_matches('.').to_string();
    if s.is_empty() {
        "file".to_string()
    } else {
        s
    }
}

/// Wysylka z tokenem: zalaczniki do repo uzytkownika, issue w `target`
/// (`owner/repo`). Wolac z watku roboczego.
pub fn send(
    token: &str,
    login: &str,
    target: &str,
    draft: &Draft,
    info: &Info,
) -> github::Result<Issue> {
    let (owner, repo) = target
        .split_once('/')
        .ok_or_else(|| spectre_sync::GitError::Other(format!("bad repo: {target}")))?;
    let mut body = draft.text.trim().to_string();
    if !draft.attachments.is_empty() {
        github::ensure_public_repo(token, login, FILES_REPO)?;
        let dir = spectre_sync::ulid::new();
        body.push_str("\n\n");
        for a in &draft.attachments {
            let name = safe_name(&a.name);
            let url = github::upload_file(
                token,
                login,
                FILES_REPO,
                &format!("{dir}/{name}"),
                &a.bytes,
                &format!("feedback attachment: {name}"),
            )?;
            if is_image(&name) {
                body.push_str(&format!("![{name}]({url})\n"));
            } else {
                body.push_str(&format!("[{name}]({url})\n"));
            }
        }
    }
    body.push_str("\n\n");
    body.push_str(&info.markdown());
    github::create_issue(token, owner, repo, &title_of(draft), &body)
}

/// Formularz nowego issue w przegladarce z wpisana trescia (bez logowania).
pub fn browser_url(target: &str, draft: &Draft, info: &Info) -> String {
    let body = format!("{}\n\n{}", draft.text.trim(), info.markdown());
    format!(
        "https://github.com/{target}/issues/new?title={}&body={}",
        github::url_encode(&title_of(draft)),
        github::url_encode(&body)
    )
}

// ----- easter egg: scenariusz paska postepu --------------------------------------

/// Krok scenariusza: komentarz, do jakiego postepu (0..1) pasek dojezdza
/// i w ile milisekund. Cofniecia sa celowe.
struct Step {
    text: &'static str,
    to: f32,
    ms: u64,
}

const SCRIPT: &[Step] = &[
    Step { text: "Folding your feedback into a paper plane…", to: 0.14, ms: 900 },
    Step { text: "Throwing it at GitHub…", to: 0.31, ms: 800 },
    Step { text: "It bounced off the Octocat.", to: 0.22, ms: 650 },
    Step { text: "Throwing again, harder…", to: 0.48, ms: 900 },
    Step { text: "Attaching pixels one by one…", to: 0.66, ms: 1100 },
    Step { text: "Dropped a few pixels. Picking them up…", to: 0.57, ms: 700 },
    Step { text: "Convincing the server this is important…", to: 0.84, ms: 1000 },
    Step { text: "Server says: \"hmm.\"", to: 0.80, ms: 500 },
    Step { text: "Server says: \"…fine.\"", to: 0.96, ms: 700 },
    Step { text: "Almost there. Really. Almost.", to: 0.99, ms: 900 },
];

/// Po scenariuszu bez odpowiedzi GitHuba: tyle czekamy, zanim przyznamy sie,
/// ze to juz nie zart.
const HONEST_AFTER_MS: u64 = 4000;

/// Postep i komentarz po `ms` od startu; `true` = scenariusz odegrany.
pub fn progress_at(ms: u64) -> (f32, &'static str, bool) {
    let mut t = 0u64;
    let mut from = 0.0f32;
    for s in SCRIPT {
        if ms < t + s.ms {
            let f = (ms - t) as f32 / s.ms as f32;
            return (from + (s.to - from) * f, s.text, false);
        }
        t += s.ms;
        from = s.to;
    }
    let text = if ms > t + HONEST_AFTER_MS {
        "OK, honestly: still waiting for GitHub…"
    } else {
        SCRIPT[SCRIPT.len() - 1].text
    };
    (from, text, true)
}

/// Dlugosc scenariusza (do testow).
#[cfg(test)]
fn script_ms() -> u64 {
    SCRIPT.iter().map(|s| s.ms).sum()
}

// ----- okno ----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Tlo okna - zjada dotkniecie.
    Panel,
    AddFile,
    Screenshot,
    Remove(usize),
    Send,
    Close,
    OpenIssue,
    Retry,
}

pub enum Phase {
    Edit,
    Sending {
        since: Instant,
        result: Option<Result<Issue, String>>,
    },
    Sent(Issue),
    Failed(String),
}

const DIALOG_W: f32 = 560.0;
const TEXT_H: f32 = 200.0;
const ROW_H: f32 = 36.0;
const BTN_H: f32 = 36.0;
const PAD: f32 = 18.0;
const LINE_H: f32 = 20.0;
const SCRIM: Rgba = Rgba {
    r: 0,
    g: 0,
    b: 0,
    a: 150,
};
const DANGER: Rgba = Rgba::rgb(190, 90, 80);

pub struct Feedback {
    pub open: bool,
    pub draft: Draft,
    pub phase: Phase,
    /// Zalogowany: wysylka w aplikacji, zalaczniki dostepne.
    pub signed_in: bool,
    /// Podpis pod tytulem, dokad idzie zgloszenie.
    pub target: String,
    hot: Option<Hit>,
    rows: Vec<(Hit, Rect)>,
    rect: Rect,
    scale: f32,
    win: (f32, f32),
}

impl Feedback {
    pub fn new(target: &str) -> Self {
        Self {
            open: false,
            draft: Draft::default(),
            phase: Phase::Edit,
            signed_in: false,
            target: target.to_string(),
            hot: None,
            rows: Vec::new(),
            rect: Rect::ZERO,
            scale: 1.0,
            win: (0.0, 0.0),
        }
    }

    pub fn layout(&mut self, w: f32, h: f32, scale: f32) {
        self.scale = scale;
        self.win = (w, h);
    }

    pub fn sending(&self) -> bool {
        matches!(self.phase, Phase::Sending { .. })
    }

    /// Otwarcie: szkic zostaje z poprzedniego razu (nie wyslany), wynik
    /// poprzedniej wysylki znika.
    pub fn show(&mut self) {
        self.open = true;
        if matches!(self.phase, Phase::Sent(_)) {
            self.phase = Phase::Edit;
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        self.hot = None;
    }

    pub fn begin_send(&mut self) {
        self.phase = Phase::Sending {
            since: Instant::now(),
            result: None,
        };
    }

    /// Wynik z watku roboczego. Blad konczy od razu; sukces czeka, az
    /// scenariusz paska sie odegra (`tick`).
    pub fn result(&mut self, r: Result<Issue, String>) {
        match (&mut self.phase, r) {
            (Phase::Sending { .. }, Err(e)) => self.phase = Phase::Failed(e),
            (Phase::Sending { result, .. }, Ok(i)) => *result = Some(Ok(i)),
            (_, Ok(i)) => self.phase = Phase::Sent(i),
            (_, Err(e)) => self.phase = Phase::Failed(e),
        }
    }

    /// Klatka animacji; `true` = nadal trwa (timer ma tykac dalej).
    pub fn tick(&mut self) -> bool {
        if let Phase::Sending { since, result } = &mut self.phase {
            let (_, _, done) = progress_at(since.elapsed().as_millis() as u64);
            if done {
                if let Some(Ok(i)) = result.take() {
                    self.draft = Draft::default();
                    self.phase = Phase::Sent(i);
                    return false;
                }
            }
            return true;
        }
        false
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.open && self.rect.contains(x, y)
    }

    /// Co jest pod dotknieciem; `None` = poza oknem (zamyka je, gdy nic
    /// nie trwa).
    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        if !self.contains(x, y) {
            return None;
        }
        Some(
            self.rows
                .iter()
                .find(|(_, r)| r.contains(x, y))
                .map(|(h, _)| *h)
                .unwrap_or(Hit::Panel),
        )
    }

    pub fn hover(&mut self, x: f32, y: f32) -> bool {
        let hot = match self.hit(x, y) {
            Some(Hit::Panel) | None => None,
            h => h,
        };
        let changed = hot != self.hot;
        self.hot = hot;
        changed
    }

    pub fn build(&mut self, out: &mut Vec<UiPrim>) {
        if !self.open {
            return;
        }
        let k = self.scale;
        self.rows.clear();
        let (ww, wh) = self.win;
        let w = (DIALOG_W * k).min(ww - 32.0 * k);
        let h = self.height() * k;
        let r = Rect {
            x: ((ww - w) * 0.5).round(),
            y: ((wh - h) * 0.5).round().max(8.0 * k),
            w,
            h,
        };
        self.rect = r;
        out.push(UiPrim::Rect {
            x: 0.0,
            y: 0.0,
            w: ww,
            h: wh,
            color: SCRIM,
            r: 0.0,
        });
        out.push(UiPrim::Rect {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            color: BG,
            r: 12.0 * k,
        });
        out.push(UiPrim::Outline {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            color: LINE,
            width: 1.0,
            r: 12.0 * k,
        });
        let pad = PAD * k;
        let inner = Rect {
            x: r.x + pad,
            y: r.y + pad,
            w: r.w - 2.0 * pad,
            h: r.h - 2.0 * pad,
        };
        let mut y = inner.y;
        match &self.phase {
            Phase::Edit => y = self.build_edit(inner, y, out),
            Phase::Sending { since, .. } => {
                let ms = since.elapsed().as_millis() as u64;
                self.build_sending(inner, y, ms, out);
            }
            Phase::Sent(issue) => {
                let issue = issue.clone();
                self.build_sent(inner, y, &issue, out);
            }
            Phase::Failed(e) => {
                let e = e.clone();
                self.build_failed(inner, y, &e, out);
            }
        }
        let _ = y;
    }

    /// Wysokosc okna (px logiczne) dla biezacej fazy.
    fn height(&self) -> f32 {
        match &self.phase {
            Phase::Edit => {
                let n = self.draft.attachments.len() as f32;
                PAD * 2.0 + 30.0 + LINE_H + 10.0 + TEXT_H + 10.0 + n * ROW_H + BTN_H + 10.0
                    + BTN_H
            }
            Phase::Sending { .. } => PAD * 2.0 + 30.0 + LINE_H + 24.0 + 10.0 + LINE_H,
            Phase::Sent(_) => PAD * 2.0 + 30.0 + LINE_H * 2.0 + 16.0 + BTN_H,
            Phase::Failed(_) => PAD * 2.0 + 30.0 + LINE_H * 3.0 + 16.0 + BTN_H,
        }
    }

    fn title(&self, inner: Rect, y: f32, text: &str, out: &mut Vec<UiPrim>) -> f32 {
        let k = self.scale;
        out.push(UiPrim::Text {
            x: inner.x,
            y,
            w: inner.w,
            h: 30.0 * k,
            text: text.to_string(),
            color: FG,
            font: UiFont::Ui,
        });
        30.0 * k
    }

    fn line(&self, inner: Rect, y: f32, text: &str, color: Rgba, out: &mut Vec<UiPrim>) -> f32 {
        let k = self.scale;
        out.push(UiPrim::Text {
            x: inner.x,
            y,
            w: inner.w,
            h: LINE_H * k,
            text: text.to_string(),
            color,
            font: UiFont::Ui,
        });
        LINE_H * k
    }

    /// Przycisk o zadanej szerokosci; `dim` = nieaktywny (bez podswietlenia).
    fn button(&mut self, r: Rect, hit: Hit, label: &str, dim: bool, out: &mut Vec<UiPrim>) {
        let k = self.scale;
        let hot = !dim && self.hot == Some(hit);
        if hot {
            out.push(UiPrim::Rect {
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
                color: HOT,
                r: 8.0 * k,
            });
        }
        out.push(UiPrim::Outline {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            color: if hot { FG_DIM } else { LINE },
            width: 1.0,
            r: 8.0 * k,
        });
        out.push(UiPrim::Text {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            text: label.to_string(),
            color: if dim { FG_DIM } else { FG },
            font: UiFont::Center,
        });
        if !dim {
            self.rows.push((hit, r));
        }
    }

    /// Rzad przyciskow rownej szerokosci.
    fn buttons(
        &mut self,
        inner: Rect,
        y: f32,
        items: &[(Hit, &str, bool)],
        out: &mut Vec<UiPrim>,
    ) -> f32 {
        let k = self.scale;
        let gap = 10.0 * k;
        let n = items.len() as f32;
        let w = (inner.w - gap * (n - 1.0)) / n;
        for (i, (hit, label, dim)) in items.iter().enumerate() {
            let r = Rect {
                x: inner.x + i as f32 * (w + gap),
                y,
                w,
                h: BTN_H * k,
            };
            self.button(r, *hit, label, *dim, out);
        }
        BTN_H * k
    }

    fn build_edit(&mut self, inner: Rect, mut y: f32, out: &mut Vec<UiPrim>) -> f32 {
        let k = self.scale;
        y += self.title(inner, y, "Send feedback", out);
        let sub = if self.signed_in {
            format!("Creates a public issue in {} - thank you!", self.target)
        } else {
            "Not signed in: opens GitHub in your browser with the text filled in".to_string()
        };
        y += self.line(inner, y, &sub, FG_DIM, out);
        y += 10.0 * k;

        // Pole tekstowe: zawijane, z kursorem na koncu; gdy tekst nie miesci
        // sie w polu, pokazujemy jego koniec (BodyTail).
        let tb = Rect {
            x: inner.x,
            y,
            w: inner.w,
            h: TEXT_H * k,
        };
        out.push(UiPrim::Outline {
            x: tb.x,
            y: tb.y,
            w: tb.w,
            h: tb.h,
            color: ACCENT,
            width: 1.0,
            r: 8.0 * k,
        });
        let text_r = Rect {
            x: tb.x + 10.0 * k,
            y: tb.y + 8.0 * k,
            w: tb.w - 20.0 * k,
            h: tb.h - 16.0 * k,
        };
        out.push(UiPrim::Clip {
            x: text_r.x,
            y: text_r.y,
            w: text_r.w,
            h: text_r.h,
        });
        let (text, color, font) = if self.draft.text.is_empty() {
            (
                "What happened? What did you expect? Steps to repeat it help a lot.".to_string(),
                FG_DIM,
                UiFont::Body,
            )
        } else {
            let font = if estimate_lines(&self.draft.text, text_r.w, k)
                > (text_r.h / (LINE_H * k)).floor() as usize
            {
                UiFont::BodyTail
            } else {
                UiFont::Body
            };
            (format!("{}|", self.draft.text), FG, font)
        };
        out.push(UiPrim::Text {
            x: text_r.x,
            y: text_r.y,
            w: text_r.w,
            h: text_r.h,
            text,
            color,
            font,
        });
        out.push(UiPrim::Unclip);
        self.rows.push((Hit::Panel, tb));
        y += tb.h + 10.0 * k;

        // Zalaczniki: nazwa, rozmiar, "×" do usuniecia.
        let attachments: Vec<(String, usize)> = self
            .draft
            .attachments
            .iter()
            .map(|a| (a.name.clone(), a.bytes.len()))
            .collect();
        for (i, (name, size)) in attachments.iter().enumerate() {
            let r = Rect {
                x: inner.x,
                y,
                w: inner.w,
                h: ROW_H * k,
            };
            let x_btn = Rect {
                x: r.x + r.w - ROW_H * k,
                y: r.y,
                w: ROW_H * k,
                h: r.h,
            };
            if self.hot == Some(Hit::Remove(i)) {
                out.push(UiPrim::Rect {
                    x: x_btn.x,
                    y: x_btn.y,
                    w: x_btn.w,
                    h: x_btn.h,
                    color: HOT,
                    r: 8.0 * k,
                });
            }
            out.push(UiPrim::Text {
                x: r.x + 8.0 * k,
                y: r.y,
                w: r.w - ROW_H * k - 8.0 * k,
                h: r.h,
                text: format!("📎 {name}  ({})", human_bytes(*size)),
                color: FG,
                font: UiFont::Ui,
            });
            out.push(UiPrim::Text {
                x: x_btn.x,
                y: x_btn.y,
                w: x_btn.w,
                h: x_btn.h,
                text: "×".to_string(),
                color: FG_DIM,
                font: UiFont::Big,
            });
            self.rows.push((Hit::Remove(i), x_btn));
            y += r.h;
        }

        // Zalaczanie tylko z tokenem (bez niego nie ma gdzie wgrac pliku).
        let can_attach = self.signed_in && self.draft.attached_bytes() < ATTACH_MAX;
        y += self.buttons(
            inner,
            y,
            &[
                (Hit::AddFile, "Attach a file…", !can_attach),
                (Hit::Screenshot, "Attach app screenshot", !can_attach),
            ],
            out,
        );
        y += 10.0 * k;
        let send_label = if self.signed_in {
            "Send"
        } else {
            "Open in browser"
        };
        let empty = self.draft.is_empty();
        y += self.buttons(
            inner,
            y,
            &[(Hit::Close, "Cancel", false), (Hit::Send, send_label, empty)],
            out,
        );
        y
    }

    fn build_sending(&mut self, inner: Rect, mut y: f32, ms: u64, out: &mut Vec<UiPrim>) {
        let k = self.scale;
        let (p, text, _) = progress_at(ms);
        y += self.title(inner, y, "Sending…", out);
        y += self.line(inner, y, text, FG_DIM, out);
        y += 24.0 * k;
        let bar_h = 8.0 * k;
        out.push(UiPrim::Rect {
            x: inner.x,
            y,
            w: inner.w,
            h: bar_h,
            color: LINE,
            r: bar_h * 0.5,
        });
        out.push(UiPrim::Rect {
            x: inner.x,
            y,
            w: (inner.w * p.clamp(0.0, 1.0)).max(bar_h),
            h: bar_h,
            color: ACCENT,
            r: bar_h * 0.5,
        });
        y += 10.0 * k;
        self.line(
            inner,
            y,
            &format!("{:.0} %", p * 100.0),
            FG_DIM,
            out,
        );
    }

    fn build_sent(&mut self, inner: Rect, mut y: f32, issue: &Issue, out: &mut Vec<UiPrim>) {
        let k = self.scale;
        y += self.title(inner, y, &format!("Sent - issue #{}", issue.number), out);
        y += self.line(inner, y, "Thank you! You can follow it on GitHub.", FG_DIM, out);
        y += self.line(inner, y, &issue.url, FG_DIM, out);
        y += 16.0 * k;
        self.buttons(
            inner,
            y,
            &[(Hit::Close, "Close", false), (Hit::OpenIssue, "Open on GitHub", false)],
            out,
        );
    }

    fn build_failed(&mut self, inner: Rect, mut y: f32, err: &str, out: &mut Vec<UiPrim>) {
        let k = self.scale;
        y += self.title(inner, y, "That didn't work", out);
        out.push(UiPrim::Clip {
            x: inner.x,
            y,
            w: inner.w,
            h: LINE_H * 3.0 * k,
        });
        out.push(UiPrim::Text {
            x: inner.x,
            y,
            w: inner.w,
            h: LINE_H * 3.0 * k,
            text: err.to_string(),
            color: DANGER,
            font: UiFont::Body,
        });
        out.push(UiPrim::Unclip);
        y += LINE_H * 3.0 * k + 16.0 * k;
        self.buttons(
            inner,
            y,
            &[(Hit::Close, "Close", false), (Hit::Retry, "Back to the draft", false)],
            out,
        );
    }
}

/// Ile wierszy zajmie tekst po zawinieciu - z grubsza (7,2 px na znak przy
/// 15 px Segoe UI); decyduje tylko, czy pokazac poczatek czy koniec.
fn estimate_lines(text: &str, w: f32, k: f32) -> usize {
    let per_line = ((w / (7.2 * k)).floor() as usize).max(1);
    text.split('\n')
        .map(|l| l.chars().count().div_ceil(per_line).max(1))
        .sum()
}

fn human_bytes(b: usize) -> String {
    if b < 1024 {
        format!("{b} B")
    } else if b < 1024 * 1024 {
        format!("{:.0} kB", b as f32 / 1024.0)
    } else {
        format!("{:.1} MB", b as f32 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenariusz_dojezdza_i_sie_cofa() {
        let (p0, _, done0) = progress_at(0);
        assert!(p0.abs() < 0.001 && !done0);
        // Krok 3 cofa z 0,31 do 0,22.
        let (a, _, _) = progress_at(900 + 800);
        let (b, _, _) = progress_at(900 + 800 + 650);
        assert!(a > b, "{a} {b}");
        let total = script_ms();
        let (p, text, done) = progress_at(total + 1);
        assert!(done && p >= 0.98 && p < 1.0, "{p}");
        assert_eq!(text, "Almost there. Really. Almost.");
        let (_, text, _) = progress_at(total + HONEST_AFTER_MS + 1);
        assert!(text.contains("honestly"));
        // Monotoniczny w obrebie kroku, bez skokow miedzy krokami.
        let mut prev = 0.0f32;
        for ms in (0..total).step_by(10) {
            let (p, _, _) = progress_at(ms);
            assert!((p - prev).abs() < 0.2, "skok przy {ms}: {prev} -> {p}");
            prev = p;
        }
    }

    #[test]
    fn tytul_i_adres_przegladarki() {
        let d = Draft {
            text: "\n  Pen lags after sleep\nsecond line".into(),
            attachments: vec![],
        };
        assert_eq!(title_of(&d), "Pen lags after sleep");
        let long = Draft {
            text: "x".repeat(100),
            attachments: vec![],
        };
        assert_eq!(title_of(&long).chars().count(), 81);
        assert_eq!(title_of(&Draft::default()), "Feedback");
        let url = browser_url("o/r", &d, &Info::default());
        assert!(url.starts_with("https://github.com/o/r/issues/new?title=Pen%20lags"));
        assert!(url.contains("body=Pen%20lags%20after%20sleep%0Asecond%20line"));
    }

    #[test]
    fn nazwy_i_rozmiary() {
        assert_eq!(safe_name("moje zdjęcie (1).PNG"), "moje_zdj_cie__1_.PNG");
        assert_eq!(safe_name("..."), "file");
        assert!(is_image("Shot.PNG") && !is_image("log.txt"));
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(3 * 1024 * 1024), "3.0 MB");
        assert_eq!(estimate_lines("", 200.0, 1.0), 1);
        assert_eq!(estimate_lines("a\nb\nc", 200.0, 1.0), 3);
        assert!(estimate_lines(&"x".repeat(100), 200.0, 1.0) >= 4);
    }
}
