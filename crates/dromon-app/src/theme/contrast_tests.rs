//! The contrast audit (§6.3, §6.9): every colour pair the views draw, in both themes, for all
//! three accent candidates, under no preference, increased contrast and reduce transparency,
//! in every state a pair can be in (rest, hover, press, selected, drop target, …).
//!
//! WCAG 2.x contrast of relative luminances. A translucent token is composited over what it
//! sits on as the renderer blends (source-over in sRGB), and so is a translucent foreground.
//! The rules (§6.3):
//!
//! - **text** 4.5:1: primary and secondary text, danger and accent text, labels on the accent.
//! - **tertiary** 3:1: `text-3` (hidden files, placeholders, captions, hints). §6.3 holds it to
//!   3:1 ("hidden files at `text-3` must still pass 3:1 against canvas and selection").
//! - **graphic** 3:1: icons, status dots, the checkbox's check, the progress ring.
//! - **focus** 3:1: the focus ring and the inline name field's accent outline.
//! - **info**: reported, not held to a ratio: disabled controls (WCAG exempts them), rows
//!   dimmed on purpose (cut, reconnecting), hidden files' icons, progress bars whose state
//!   their line also says in words, control outlines.
//!
//! `cargo test -p dromon-app --lib contrast -- --nocapture` prints the worst pair of every rule
//! per theme, accent and preference; a failure prints every failing pair.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Rule {
    Text,
    Tertiary,
    Graphic,
    Focus,
    Info,
}

impl Rule {
    const ALL: [Rule; 5] = [Rule::Text, Rule::Tertiary, Rule::Graphic, Rule::Focus, Rule::Info];

    fn min(self) -> f32 {
        match self {
            Rule::Text => 4.5,
            Rule::Tertiary | Rule::Graphic | Rule::Focus => 3.0,
            Rule::Info => 0.0,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Rule::Text => "text 4.5",
            Rule::Tertiary => "tertiary 3",
            Rule::Graphic => "graphic 3",
            Rule::Focus => "focus 3",
            Rule::Info => "info",
        }
    }
}

/// One pair: `fg` drawn over the stack `bg` (an opaque base, then washes over it).
struct Pair {
    rule: Rule,
    what: String,
    fg: Color,
    bg: Vec<Color>,
}

impl Pair {
    fn backdrop(&self) -> Color {
        let (base, washes) = self.bg.split_first().expect("a base surface");
        assert!((base.a - 1.0).abs() < 1e-6, "{}: the base of a stack is opaque", self.what);
        washes.iter().fold(*base, |under, wash| flatten(*wash, under))
    }

    fn ratio(&self) -> f32 {
        let bg = self.backdrop();
        ratio(flatten(self.fg, bg), bg)
    }
}

/// WCAG relative luminance: Rune keeps colours in linear sRGB already.
fn luminance(c: Color) -> f32 {
    0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b
}

fn ratio(a: Color, b: Color) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Collects the pairs of one token set.
struct Audit {
    out: Vec<Pair>,
}

impl Audit {
    fn add(&mut self, rule: Rule, what: impl Into<String>, fg: Color, bg: &[Color]) {
        self.out.push(Pair { rule, what: what.into(), fg, bg: bg.to_vec() });
    }

    /// At rest, hovered (`hover` over it) and pressed (`hover` and `press` over it, as Rune
    /// paints a held press: both overlays).
    fn states(&mut self, rule: Rule, what: &str, fg: Color, bg: &[Color], hover: Color, press: Color) {
        self.hovers(rule, what, fg, bg, hover);
        let mut h = bg.to_vec();
        h.extend([hover, press]);
        self.add(rule, format!("{what} · press"), fg, &h);
    }

    /// At rest and hovered, for what has no press fill.
    fn hovers(&mut self, rule: Rule, what: &str, fg: Color, bg: &[Color], hover: Color) {
        self.add(rule, what, fg, bg);
        let mut h = bg.to_vec();
        h.push(hover);
        self.add(rule, format!("{what} · hover"), fg, &h);
    }
}

/// A file category's tint, as `views/list.rs` (`category`) draws it.
fn category(dark: bool, hue: f32) -> Color {
    if dark { rune::oklch(0.74, 0.055, hue) } else { rune::oklch(0.55, 0.06, hue) }
}

/// The category hues `icons::for_file` hands out.
const HUES: [(&str, f32); 7] = [
    ("code", 250.0),
    ("image", 320.0),
    ("archive", 70.0),
    ("audio", 150.0),
    ("video", 200.0),
    ("pdf", 355.0),
    ("binary", 20.0),
];

/// Every pair the views draw with `t`, by where it is.
fn pairs(t: &Tokens) -> Vec<Pair> {
    let mut a = Audit { out: Vec::new() };
    list_pairs(&mut a, t);
    sidebar_pairs(&mut a, t);
    toolbar_pairs(&mut a, t);
    banner_and_states_pairs(&mut a, t);
    sheet_pairs(&mut a, t);
    menu_pairs(&mut a, t);
    transfers_pairs(&mut a, t);
    focus_ring_pairs(&mut a, t);
    a.out
}

/// The list (views/list.rs, edit.rs, drop.rs): a row at rest, hovered, pressed, selected, as a
/// drop target, under the drop wash; and the header.
fn list_pairs(a: &mut Audit, t: &Tokens) {
    use Rule::*;
    let (hv, pr) = (t.hover, t.press);
    let row_stacks: Vec<(&str, Vec<Color>)> = vec![
        ("row", vec![t.canvas]),
        ("row · hover", vec![t.canvas, hv]),
        ("row · press", vec![t.canvas, hv, pr]),
        ("row · selected", vec![t.canvas, t.selection]),
        ("row · selected · hover", vec![t.canvas, t.selection, hv]),
        ("row · selected · press", vec![t.canvas, t.selection, hv, pr]),
        ("row · drop target", vec![t.canvas, t.accent.alpha(0.22)]),
        ("row · drop target · hover", vec![t.canvas, t.accent.alpha(0.22), hv]),
        ("row · under the drop wash", vec![t.canvas, t.accent.alpha(0.06)]),
        ("row · selected · under the drop wash", vec![t.canvas, t.accent.alpha(0.06), t.selection]),
    ];
    for (state, bg) in &row_stacks {
        a.add(Text, format!("{state}: name (text)"), t.text, bg);
        a.add(Text, format!("{state}: size, date (text-2)"), t.text_2, bg);
        a.add(Tertiary, format!("{state}: hidden file (text-3)"), t.text_3, bg);
        a.add(Graphic, format!("{state}: file icon (text-2)"), t.text_2, bg);
        a.add(Graphic, format!("{state}: selected file icon (text)"), t.text, bg);
        a.add(Graphic, format!("{state}: folder icon"), t.folder, bg);
        for (name, hue) in HUES {
            a.add(Graphic, format!("{state}: {name} icon"), category(t.dark, hue), bg);
        }
        a.add(Info, format!("{state}: hidden folder icon (60%)"), t.folder.alpha(t.folder.a * 0.6), bg);
        a.add(Info, format!("{state}: hidden file icon (60%)"), t.text_2.alpha(0.6), bg);
        a.add(Info, format!("{state}: cut row name (50%)"), t.text.alpha(0.5), bg);
        a.add(Info, format!("{state}: cut row date (50%)"), t.text_2.alpha(0.5), bg);
        a.add(Info, format!("{state}: reconnecting, dimmed name (60%)"), t.text.alpha(0.6), bg);
        a.add(Info, format!("{state}: reconnecting, dimmed date (60%)"), t.text_2.alpha(0.6), bg);
    }
    a.add(Graphic, "symlink badge (text-2 on canvas)", t.text_2, &[t.canvas]);
    a.add(Focus, "name field outline (accent) on canvas", t.accent, &[t.canvas]);
    a.add(Focus, "name field outline (accent) on the selected row", t.accent, &[t.canvas, t.selection]);
    a.add(Graphic, "name field outline (danger) on canvas", t.danger, &[t.canvas]);
    a.add(Text, "name field text on its canvas fill", t.text, &[t.canvas]);
    a.add(Focus, "drop outline (accent) over its wash", t.accent, &[t.canvas, t.accent.alpha(0.06)]);
    a.states(Text, "header: sorted column (text-2)", t.text_2, &[t.canvas], hv, pr);
    a.states(Tertiary, "header: column (text-3)", t.text_3, &[t.canvas], hv, pr);
    a.add(Graphic, "header: sort caret (text-3)", t.text_3, &[t.canvas]);
}

/// The sidebar (views/sidebar.rs).
fn sidebar_pairs(a: &mut Audit, t: &Tokens) {
    use Rule::*;
    let (hv, pr) = (t.hover, t.press);
    a.add(Tertiary, "sidebar: section caption (text-3)", t.text_3, &[t.sidebar]);
    a.add(Text, "sidebar: app name (text-2)", t.text_2, &[t.sidebar]);
    a.states(Text, "sidebar: host (text-2)", t.text_2, &[t.sidebar], hv, pr);
    a.states(Graphic, "sidebar: host icon (text-3)", t.text_3, &[t.sidebar], hv, pr);
    a.states(Text, "sidebar: host on screen (text)", t.text, &[t.sidebar, t.active], hv, pr);
    a.states(Graphic, "sidebar: host on screen, icon (text-2)", t.text_2, &[t.sidebar, t.active], hv, pr);
    for (dot, color) in [("connected", t.accent), ("reconnecting", t.warning)] {
        a.states(Graphic, &format!("sidebar: {dot} dot"), color, &[t.sidebar], hv, pr);
        a.states(Graphic, &format!("sidebar: {dot} dot, host on screen"), color, &[t.sidebar, t.active], hv, pr);
    }
    a.add(Text, "sidebar: host as a drop target (text-2)", t.text_2, &[t.sidebar, t.accent.alpha(0.22)]);
    a.add(Text, "sidebar: host on screen as a drop target (text)", t.text, &[t.sidebar, t.accent.alpha(0.22)]);
    a.add(Graphic, "sidebar: drop target's connected dot", t.accent, &[t.sidebar, t.accent.alpha(0.22)]);
    a.states(Text, "sidebar: Quick connect (text-2)", t.text_2, &[t.sidebar], hv, pr);
    a.states(Graphic, "sidebar: Quick connect icon (text-3)", t.text_3, &[t.sidebar], hv, pr);
}

/// The toolbar (views/toolbar.rs, transfers.rs, widgets.rs).
fn toolbar_pairs(a: &mut Audit, t: &Tokens) {
    use Rule::*;
    let (hv, pr) = (t.hover, t.press);
    a.states(Graphic, "toolbar: icon button (text-2)", t.text_2, &[t.surface], hv, pr);
    a.add(Info, "toolbar: disabled icon button (text-3 at 60%)", t.text_3.alpha(0.6), &[t.surface]);
    a.add(Text, "toolbar: current crumb (text)", t.text, &[t.surface]);
    a.states(Text, "toolbar: crumb (text-2)", t.text_2, &[t.surface], hv, pr);
    a.add(Graphic, "toolbar: crumb chevron (text-3)", t.text_3, &[t.surface]);
    a.add(Text, "toolbar: crumb as a drop target (text-2)", t.text_2, &[t.surface, t.accent.alpha(0.22)]);
    a.add(Graphic, "toolbar: connecting spinner (text-3)", t.text_3, &[t.surface]);
    a.add(Text, "toolbar: Connecting to … (text-2)", t.text_2, &[t.surface]);
    let field = [t.surface, t.field];
    a.add(Graphic, "filter: search icon (text-3)", t.text_3, &field);
    a.add(Graphic, "filter: search icon, open (text-2)", t.text_2, &field);
    a.hovers(Tertiary, "filter: closed label (text-3)", t.text_3, &field, hv);
    a.add(Tertiary, "filter: placeholder (text-3)", t.text_3, &field);
    a.add(Text, "filter: typed text (text)", t.text, &field);
    a.add(Tertiary, "filter: ⌘F hint (text-3)", t.text_3, &field);
    a.states(Graphic, "filter: clear (text-3)", t.text_3, &field, hv, pr);
    a.add(Tertiary, "toolbar: ⌘K hint (text-3)", t.text_3, &[t.surface]);
    a.states(Graphic, "transfers button: glyph (text-2)", t.text_2, &[t.surface], hv, pr);
    a.add(Graphic, "transfers button: glyph, popover open (text-2)", t.text_2, &[t.surface, pr]);
    a.states(Graphic, "transfers button: idle glyph (text-3 at 60%)", t.text_3.alpha(0.6), &[t.surface], hv, pr);
    a.add(Graphic, "transfers button: progress ring (accent)", t.accent, &[t.surface]);
    a.add(Info, "transfers button: ring track (border-strong)", t.border_strong, &[t.surface]);
    a.add(Text, "transfers button: count badge (on-accent on accent)", t.on_accent, &[t.accent]);
}

/// The reconnecting banner and the status bar (views/states.rs, status.rs), and the list's
/// other states.
fn banner_and_states_pairs(a: &mut Audit, t: &Tokens) {
    use Rule::*;
    let (hv, pr) = (t.hover, t.press);
    let banner = [t.canvas, t.banner];
    a.add(Text, "banner: headline (text)", t.text, &banner);
    a.add(Tertiary, "banner: detail (text-3)", t.text_3, &banner);
    a.add(Graphic, "banner: warning dot", t.warning, &banner);
    a.hovers(Text, "banner: Sign in (text)", t.text, &banner, hv);
    a.add(Text, "status bar: summary (text-2)", t.text_2, &[t.surface]);
    a.add(Tertiary, "status bar: Not connected (text-3)", t.text_3, &[t.surface]);
    for (dot, color) in
        [("connected", t.accent), ("reconnecting", t.warning), ("failed", t.danger), ("connecting", t.text_3)]
    {
        a.add(Graphic, format!("status bar: {dot} dot"), color, &[t.surface]);
    }

    // The list's other states (views/states.rs).
    a.add(Text, "states: headline (text)", t.text, &[t.canvas]);
    a.add(Text, "states: sub-text (text-2)", t.text_2, &[t.canvas]);
    a.add(Tertiary, "states: denied folder's path (text-3)", t.text_3, &[t.canvas]);
    a.states(Text, "states: button label (text)", t.text, &[t.canvas, t.field], hv, pr);
    a.states(Graphic, "states: button icon (text-2)", t.text_2, &[t.canvas, t.field], hv, pr);
    a.add(Graphic, "states: lock tile (text-2)", t.text_2, &[t.canvas, t.field]);
    a.add(Info, "states: skeleton bar", t.skeleton, &[t.canvas]);
}

/// Sheets (views/sheets.rs, conflict.rs, delete.rs, quit.rs), and toasts, notices, the drop
/// pill and the name field's error bubble (on `sheet`).
fn sheet_pairs(a: &mut Audit, t: &Tokens) {
    use Rule::*;
    let (hv, pr) = (t.hover, t.press);
    a.add(Text, "sheet: title (text)", t.text, &[t.sheet]);
    a.add(Text, "sheet: prompt (text-2)", t.text_2, &[t.sheet]);
    a.add(Tertiary, "sheet: memory-only note (text-3)", t.text_3, &[t.sheet]);
    a.add(Text, "sheet: typed text in the field (text)", t.text, &[t.sheet, t.field]);
    a.add(Text, "sheet: error (danger)", t.danger, &[t.sheet]);
    a.add(Graphic, "sheet: field outline, error (danger)", t.danger, &[t.sheet, t.field]);
    a.add(Info, "sheet: field outline (border-strong)", t.border_strong, &[t.sheet, t.field]);
    a.add(Tertiary, "sheet: fingerprint caption (text-3 on raised)", t.text_3, &[t.raised]);
    a.add(Text, "sheet: fingerprint (text on raised)", t.text, &[t.raised]);
    a.states(Text, "sheet: secondary button (text)", t.text, &[t.sheet], hv, pr);
    a.add(Text, "sheet: primary button (on-accent on accent)", t.on_accent, &[t.accent]);
    // Busy: the answered primary button, inactive until ssh moves on (WCAG exempts it).
    a.add(Info, "sheet: busy button (on-accent on accent at 60%)", t.on_accent, &[t.sheet, t.accent.alpha(0.6)]);
    a.add(Text, "log: lines (text-2 on raised)", t.text_2, &[t.raised]);
    a.add(Tertiary, "log: nothing yet (text-3 on raised)", t.text_3, &[t.raised]);
    a.states(Text, "danger button (danger)", t.danger, &[t.sheet], t.danger.alpha(0.08), t.danger.alpha(0.14));
    a.add(Info, "danger button outline (danger at 55%)", t.danger.alpha(0.55), &[t.sheet]);
    a.add(Tertiary, "conflict: Existing/Incoming caption (text-3 on surface)", t.text_3, &[t.surface]);
    a.add(Text, "conflict: size, newer date (text on surface)", t.text, &[t.surface]);
    a.add(Text, "conflict: older date (text-2 on surface)", t.text_2, &[t.surface]);
    a.add(Text, "conflict: Apply to all (text)", t.text, &[t.sheet]);
    a.add(Graphic, "conflict: checkbox check (on-accent on accent)", t.on_accent, &[t.accent]);
    a.add(Info, "conflict: checkbox outline (border-strong)", t.border_strong, &[t.sheet]);
    a.add(Graphic, "delete: names' icons (text-2 on raised)", t.text_2, &[t.raised]);
    a.add(Text, "delete: names (text on raised)", t.text, &[t.raised]);
    a.add(Tertiary, "delete: and N more (text-3 on raised)", t.text_3, &[t.raised]);

    // Toasts, notices, the drop pill, the name field's error bubble (on `sheet`).
    a.add(Text, "toast: text", t.text, &[t.sheet]);
    a.add(Graphic, "toast: spinner, folder (text-2)", t.text_2, &[t.sheet]);
    a.add(Graphic, "toast: warning (danger)", t.danger, &[t.sheet]);
    a.states(Graphic, "toast: dismiss (text-3)", t.text_3, &[t.sheet], hv, pr);
    a.add(Graphic, "drop pill: icon (accent)", t.accent, &[t.sheet]);
    a.add(Text, "drop pill: text", t.text, &[t.sheet]);
    a.add(Text, "name field error bubble (danger)", t.danger, &[t.sheet]);
}

/// The drawn context menu (views/menu.rs).
fn menu_pairs(a: &mut Audit, t: &Tokens) {
    use Rule::*;
    let (hv, pr) = (t.hover, t.press);
    a.states(Text, "menu: item (text)", t.text, &[t.raised], hv, pr);
    a.states(Text, "menu: Delete (danger)", t.danger, &[t.raised], hv, pr);
    a.states(Tertiary, "menu: shortcut (text-3)", t.text_3, &[t.raised], hv, pr);
    a.states(Graphic, "menu: check (text-2)", t.text_2, &[t.raised], hv, pr);
    a.add(Info, "menu: disabled item (text-3)", t.text_3, &[t.raised]);
}

/// The transfers popover (views/transfers.rs).
fn transfers_pairs(a: &mut Audit, t: &Tokens) {
    use Rule::*;
    let (hv, pr) = (t.hover, t.press);
    let track = [t.raised, t.field];
    a.add(Text, "transfers: open tab (text)", t.text, &[t.raised, t.field, pr]);
    a.add(Tertiary, "transfers: open tab's count (text-3)", t.text_3, &[t.raised, t.field, pr]);
    a.hovers(Text, "transfers: other tab (text-2)", t.text_2, &track, hv);
    a.hovers(Tertiary, "transfers: other tab's count (text-3)", t.text_3, &track, hv);
    a.add(Text, "transfers: No downloads yet (text-2)", t.text_2, &[t.raised]);
    a.add(Tertiary, "transfers: empty hint (text-3)", t.text_3, &[t.raised]);
    for (state, bg) in [("job", vec![t.raised]), ("job · hover", vec![t.raised, hv])] {
        let mut tile = bg.clone();
        tile.push(t.field);
        a.add(Graphic, format!("{state}: tile glyph (text-2)"), t.text_2, &tile);
        a.add(Graphic, format!("{state}: done check (accent)"), t.accent, &tile);
        a.add(Graphic, format!("{state}: failed warning (danger)"), t.danger, &tile);
        a.add(Text, format!("{state}: title (text)"), t.text, &bg);
        a.add(Text, format!("{state}: progress line (text-2)"), t.text_2, &bg);
        a.add(Text, format!("{state}: failed line (danger)"), t.danger, &bg);
        let mut bar = bg.clone();
        bar.push(t.border);
        a.add(Info, format!("{state}: progress bar (accent) on its track"), t.accent, &bar);
        a.add(Info, format!("{state}: failed bar (danger) on its track"), t.danger, &bar);
        a.add(Info, format!("{state}: paused bar (text-3) on its track"), t.text_3, &bar);
        a.add(Text, format!("{state}: Resolve… (accent)"), t.accent, &bg);
        let (mut on, mut down) = (bg.clone(), bg.clone());
        on.push(t.accent.alpha(0.1));
        down.extend([t.accent.alpha(0.1), t.accent.alpha(0.16)]);
        a.add(Text, format!("{state}: Resolve… (accent) · hover"), t.accent, &on);
        a.add(Text, format!("{state}: Resolve… (accent) · press"), t.accent, &down);
    }
    a.states(Graphic, "job · hover: pause, cancel, retry (text-2)", t.text_2, &[t.raised, hv], hv, pr);
    a.add(Tertiary, "transfers: N active (text-3)", t.text_3, &[t.raised]);
    a.states(Text, "transfers: Clear finished (text-2)", t.text_2, &[t.raised], hv, pr);
    a.add(Info, "transfers: Clear finished, disabled (text-3 at 60%)", t.text_3.alpha(0.6), &[t.raised]);
}

/// The keyboard focus ring (accent, 2 pt) on every surface it can sit on.
fn focus_ring_pairs(a: &mut Audit, t: &Tokens) {
    for (name, bg) in [
        ("canvas", vec![t.canvas]),
        ("selected row", vec![t.canvas, t.selection]),
        ("sidebar", vec![t.sidebar]),
        ("toolbar", vec![t.surface]),
        ("popover", vec![t.raised]),
        ("sheet", vec![t.sheet]),
    ] {
        a.add(Rule::Focus, format!("focus ring on the {name}"), t.focus, &bg);
    }
}

/// Pairs that fail and that no token nudge should fix, because the fix is a design decision
/// (reported to the plan, `.plans/progress.md`): the reason, or `None` for a pair the audit
/// holds to its rule.
fn design_exception(what: &str) -> Option<&'static str> {
    let on_accent_fill = what.contains("selected") || what.contains("drop");
    if what.starts_with("transfers button: idle glyph") {
        Some(
            "the idle transfers glyph is drawn as quiet as a disabled button on purpose (§6.8), though it opens the popover",
        )
    } else if what.contains("Resolve…") && (what.ends_with("· hover") || what.ends_with("· press")) {
        Some("accent text on its own accent wash: Resolve… would need a neutral or filled hover")
    } else if what.contains("folder icon") && on_accent_fill && !what.contains("hidden") {
        Some("accent glyph on an accent fill: folders on selected rows and drop targets would need another tint")
    } else if what.contains("drop target's connected dot") {
        Some("accent dot on an accent fill")
    } else if what.starts_with("danger button (danger) ·") {
        Some("danger text on its own danger wash: the danger button would need a neutral or filled hover and press")
    } else {
        None
    }
}

/// One audited token set: which accent, theme and preference.
struct Case {
    accent: Accent,
    dark: bool,
    mode: &'static str,
    t: Tokens,
}

fn cases() -> Vec<Case> {
    let modes = [
        ("default", Access::default()),
        ("increase contrast", Access { increase_contrast: true, ..Access::default() }),
        ("reduce transparency", Access { reduce_transparency: true, ..Access::default() }),
    ];
    let mut out = Vec::new();
    for accent in ACCENTS {
        for dark in [true, false] {
            for (mode, access) in modes {
                out.push(Case { accent, dark, mode, t: build(dark, accent, access) });
            }
        }
    }
    out
}

fn row(case: &Case, rule: &str, r: f32, p: &Pair) -> String {
    let theme = if case.dark { "dark" } else { "light" };
    format!(
        "{:<10} {:<6} {:<20} {:<11} {:>6.2}  {} ({} on {})",
        case.accent.name,
        theme,
        case.mode,
        rule,
        r,
        p.what,
        p.fg.hex(),
        p.backdrop().hex()
    )
}

#[test]
fn every_pair_meets_its_contrast_rule_in_every_theme_accent_and_mode() {
    let head = format!("{:<10} {:<6} {:<20} {:<11} {:>6}  pair", "accent", "theme", "mode", "rule", "ratio");
    let (mut summary, mut exceptions, mut failures) = (vec![head.clone()], vec![head.clone()], vec![head]);
    for case in cases() {
        let pairs = pairs(&case.t);
        let held = |p: &&Pair| p.rule == Rule::Info || design_exception(&p.what).is_none();
        // The worst pair of each rule that the audit holds to it (info: of everything reported).
        for rule in Rule::ALL {
            let worst = pairs
                .iter()
                .filter(held)
                .filter(|p| p.rule == rule)
                .map(|p| (p.ratio(), p))
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((r, p)) = worst {
                summary.push(row(&case, rule.name(), r, p));
            }
        }
        for p in pairs.iter().filter(|p| p.rule != Rule::Info) {
            let r = p.ratio();
            match design_exception(&p.what) {
                Some(why) if r < p.rule.min() => {
                    exceptions.push(format!("{}\n    {why}", row(&case, p.rule.name(), r, p)))
                }
                Some(_) => {}
                None if r < p.rule.min() => failures.push(row(&case, p.rule.name(), r, p)),
                None => {}
            }
        }
    }
    println!("Worst pair per rule:\n{}\n", summary.join("\n"));
    println!("Below their rule, left for a design decision:\n{}\n", exceptions.join("\n"));
    assert!(failures.len() == 1, "{} pairs fail their contrast rule:\n{}", failures.len() - 1, failures.join("\n"));
}

#[test]
fn the_audit_measures_as_wcag_does() {
    assert!((ratio(Color::BLACK, Color::WHITE) - 21.0).abs() < 1e-3);
    // #767676 on white is the textbook 4.54:1.
    assert!((ratio(c("#767676"), Color::WHITE) - 4.54).abs() < 0.01);
    // Half-transparent black over white is the sRGB midpoint, 0.5 (3.98:1; #808080 is 3.95).
    let p = Pair { rule: Rule::Text, what: "half black".into(), fg: Color::BLACK.alpha(0.5), bg: vec![Color::WHITE] };
    assert!((p.ratio() - 3.98).abs() < 0.01, "{}", p.ratio());
}
