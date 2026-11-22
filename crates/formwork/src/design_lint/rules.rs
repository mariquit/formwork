// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>

//! Rules over browser-resolved elements.
//!
//! Every rule here reports *measured* evidence (a ratio, a pixel value, a
//! resolved color) rather than a restatement of the rule, so a finding can be
//! acted on without re-deriving it. Rules abstain whenever the input is
//! ambiguous — a false finding costs more than a miss, because it blocks a
//! legitimate design.

use super::cdp::Element;
use super::color::{self, contrast_ratio, is_large_text, Rgba};
use super::{Finding, Severity};
use std::collections::HashMap;
use std::path::Path;

/// Display faces so common in generated UI that they read as a default rather
/// than a choice.
pub const DEFAULT_FACES: [&str; 8] = [
    "inter",
    "playfair display",
    "fraunces",
    "cormorant",
    "space grotesk",
    "dm sans",
    "outfit",
    "plus jakarta sans",
];

/// Tokens declared by `.formwork/design-system.md`. Drift from these is only
/// checked when the file actually declares them — an undeclared category
/// abstains rather than flagging everything.
#[derive(Debug, Default, Clone)]
pub struct Tokens {
    pub colors: Vec<Rgba>,
    pub fonts: Vec<String>,
    pub radii: Vec<String>,
}

impl Tokens {
    pub fn has_any(&self) -> bool {
        !self.colors.is_empty() || !self.fonts.is_empty() || !self.radii.is_empty()
    }

    /// Extract tokens from `.formwork/design-system.md`.
    ///
    /// Deliberately format-tolerant: any `#rrggbb` / `rgb()` literal counts as
    /// a declared color, any `font-family:`-ish line contributes families, and
    /// any `NNpx`/`Nrem` next to a radius word contributes a radius. The file
    /// is prose written by an agent, so demanding rigid YAML would make the
    /// gate fail on formatting rather than on craft.
    pub fn from_design_system(workspace: &Path) -> Option<Tokens> {
        let path = workspace.join(crate::design_paths::DESIGN_SYSTEM_PATH);
        let text = std::fs::read_to_string(path).ok()?;
        let mut t = Tokens::default();

        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            let lower = line.to_ascii_lowercase();

            // Colors: every hex/rgb literal anywhere in the file.
            t.colors.extend(color::hex_literals(line));
            if let Some(p) = lower.find("rgb(").or_else(|| lower.find("rgba(")) {
                // `p` is a byte offset into `lower`, which is byte-aligned with
                // `line` (ASCII-lowercasing never changes byte length). Slice
                // via `get` and search within the remainder so a missing `)`
                // or a boundary can never panic.
                if let Some(tail) = line.get(p..) {
                    if let Some(rel) = tail.find(')') {
                        if let Some(frag) = tail.get(..=rel) {
                            if let Some(c) = color::parse(frag) {
                                t.colors.push(c);
                            }
                        }
                    }
                }
            }

            // Fonts: quoted or comma-listed families on a typography-ish line.
            if lower.contains("font") || lower.contains("typeface") || lower.contains("family") {
                for part in line
                    .split(&[':', '—', '-', '|'][..])
                    .skip(1)
                    .flat_map(|s| s.split(','))
                {
                    let fam = part
                        .trim()
                        .trim_matches(|c: char| c == '"' || c == '\'' || c == '`' || c == '*')
                        .trim();
                    if fam.is_empty() || fam.len() > 40 {
                        continue;
                    }
                    // Skip prose words that aren't plausibly a family name.
                    if fam.split_whitespace().count() > 4 {
                        continue;
                    }
                    if fam.chars().any(|c| c.is_ascii_digit()) {
                        continue;
                    }
                    t.fonts.push(fam.to_ascii_lowercase());
                }
            }

            // Radii: values on a radius/rounded line.
            if lower.contains("radius") || lower.contains("rounded") {
                for tok in line.split_whitespace() {
                    let v = tok.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.');
                    if (v.ends_with("px") || v.ends_with("rem")) && v.len() <= 8 {
                        t.radii.push(v.to_ascii_lowercase());
                    }
                }
            }
        }

        t.colors.dedup();
        t.fonts.sort();
        t.fonts.dedup();
        t.radii.sort();
        t.radii.dedup();
        if t.has_any() {
            Some(t)
        } else {
            None
        }
    }

    pub fn allows_color(&self, c: Rgba) -> bool {
        // Neutrals are always allowed: a design system names its palette, not
        // every gray in its scale, and flagging neutrals would bury real drift.
        if !c.has_chroma(24) {
            return true;
        }
        self.colors.iter().any(|t| {
            let close = |a: u8, b: u8| (a as i16 - b as i16).abs() <= 12;
            close(t.r, c.r) && close(t.g, c.g) && close(t.b, c.b)
        })
    }

    fn allows_font(&self, family: &str) -> bool {
        if self.fonts.is_empty() {
            return true;
        }
        let first = first_family(family);
        if first.is_empty() {
            return true;
        }
        self.fonts.iter().any(|f| f == &first || f.contains(&first))
    }
}

/// The first real family in a `font-family` list, lowercased.
fn first_family(list: &str) -> String {
    list.split(',')
        .map(|s| s.trim().trim_matches(|c| c == '"' || c == '\'').trim())
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn generic_family(name: &str) -> bool {
    matches!(
        name,
        "sans-serif"
            | "serif"
            | "monospace"
            | "system-ui"
            | "ui-sans-serif"
            | "ui-serif"
            | "ui-monospace"
            | "cursive"
            | "fantasy"
            | "-apple-system"
            | "blinkmacsystemfont"
            | "inherit"
            | "initial"
    )
}

/// True when the element carries real, visible text of its own.
fn has_own_text(e: &Element) -> bool {
    !e.own_text.trim().is_empty() && e.own_text.trim().chars().any(|c| c.is_alphanumeric())
}

/// Card-ish: a block with its own surface treatment. Used by nested-cards.
fn is_card(e: &Element) -> bool {
    if !matches!(e.tag.as_str(), "div" | "section" | "article" | "li" | "a") {
        return false;
    }
    let has_radius = e
        .border_radius
        .split_whitespace()
        .filter_map(|v| v.trim_end_matches("px").parse::<f64>().ok())
        .any(|v| v >= 6.0);
    let has_bg = color::parse(&e.background)
        .map(|c| c.a > 0.0)
        .unwrap_or(false);
    let has_shadow = e.box_shadow != "none" && !e.box_shadow.is_empty();
    has_bg && (has_radius || has_shadow) && e.width >= 80.0 && e.height >= 40.0
}

/// Zero-offset colored glow: a chromatic halo with no offset is decoration
/// pretending to be elevation. A real shadow has offset or is neutral.
fn is_glow(shadow: &str) -> Option<String> {
    if shadow == "none" || shadow.is_empty() {
        return None;
    }
    for layer in split_shadow_layers(shadow) {
        let c = leading_color(&layer)?;
        // Numbers after the color: offset-x, offset-y, blur, spread.
        let nums: Vec<f64> = layer
            .split_whitespace()
            .filter_map(|t| t.trim_end_matches("px").parse::<f64>().ok())
            .collect();
        if nums.len() < 3 {
            continue;
        }
        let (ox, oy, blur) = (nums[0], nums[1], nums[2]);
        if ox.abs() < 0.5 && oy.abs() < 0.5 && blur > 2.0 && c.has_chroma(40) && c.a > 0.15 {
            return Some(c.hex());
        }
    }
    None
}

/// Split a `box-shadow` value on commas that separate layers (not inside
/// `rgb(...)`).
fn split_shadow_layers(v: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for ch in v.chars() {
        match ch {
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// The color at the start of a computed shadow layer (Chrome puts it first).
///
/// Defensive by construction: a computed value is well-formed, but this also
/// runs on the static engine's raw source, where `)` can appear before `(`.
/// Every slice below is bounds- and order-checked so no malformed value panics.
fn leading_color(layer: &str) -> Option<Rgba> {
    let t = layer.trim();
    if let Some(open) = t.find('(') {
        let head = &t[..open];
        if head.ends_with("rgb") || head.ends_with("rgba") {
            // The close paren must come after the open paren.
            let close = t[open..].find(')').map(|rel| open + rel)?;
            let start = head.rfind(char::is_whitespace).map_or(0, |i| i + 1);
            // start indexes into `head` (< open <= close), so start <= close.
            return t.get(start..=close).and_then(color::parse);
        }
    }
    // Otherwise the leading token itself may be a hex/named color.
    t.split_whitespace().next().and_then(color::parse)
}

/// Run every element rule. `tokens` enables design-system drift checks.
pub fn run(elements: &[Element], tokens: Option<&Tokens>) -> Vec<Finding> {
    let mut f = Vec::new();
    contrast(elements, &mut f);
    side_tab(elements, &mut f);
    glow(elements, &mut f);
    nested_cards(elements, &mut f);
    gradient_text(elements, &mut f);
    kicker(elements, &mut f);
    typography(elements, &mut f);
    heading_order(elements, &mut f);
    tiny_text(elements, &mut f);
    measure(elements, &mut f);
    touch_targets(elements, &mut f);
    overflow(elements, &mut f);
    semantic_button(elements, &mut f);
    placeholder_copy(elements, &mut f);
    decorative_blur(elements, &mut f);
    if let Some(t) = tokens {
        token_drift(elements, t, &mut f);
    }
    f
}

/// WCAG AA contrast. This is a **blocking** rule: unlike the aesthetic rules,
/// it reports an objective, measurable failure — the text either meets the
/// floor or it doesn't, and shipping unreadable text is a defect rather than a
/// matter of taste.
///
/// Because it blocks, every ambiguous case must abstain. A false positive here
/// rejects a legitimate design, so the rule only fires when the foreground and
/// the painted backdrop are both known with certainty.
fn contrast(els: &[Element], out: &mut Vec<Finding>) {
    for e in els {
        if !has_own_text(e) {
            continue;
        }
        // Not visually present: screen-reader-only text, or an element faded
        // out (a pre-animation state, or a deliberately dimmed layer).
        if e.sr_only || e.opacity < 0.95 {
            continue;
        }
        // Gradient-clipped text is painted from the background, so the computed
        // `color` is not what the user sees. Abstain.
        if e.background_image.contains("gradient") && e.color.contains("rgba(0, 0, 0, 0)") {
            continue;
        }
        // A background image (photo, gradient, mesh) sits behind this text, so
        // the real backdrop is not a color we can read.
        if e.bg_uncertain {
            continue;
        }
        let Some(fg_raw) = color::parse(&e.color) else {
            continue;
        };
        let Some(bg) = color::parse(&e.effective_bg) else {
            continue;
        };
        if fg_raw.a <= 0.05 {
            continue;
        }
        // A translucent foreground composited over a *known* backdrop is fine
        // to measure; `over` resolves it. Anything else already abstained.
        let fg = fg_raw.over(bg);
        let ratio = contrast_ratio(fg, bg);
        let threshold = if is_large_text(e.font_size_px, e.font_weight) {
            3.0
        } else {
            4.5
        };
        if ratio < threshold {
            out.push(Finding {
                rule: "low-contrast",
                severity: Severity::Major,
                location: e.location(),
                evidence: format!(
                    "{:.1}:1 (need {:.1}:1) — {} on {} at {:.0}px",
                    ratio,
                    threshold,
                    fg.hex(),
                    bg.hex(),
                    e.font_size_px
                ),
                remedy: "Darken the text or lighten the surface until it meets the WCAG AA floor (4.5:1 body, 3:1 large text).",
            });
        }
    }
}

fn side_tab(els: &[Element], out: &mut Vec<Finding>) {
    for e in els {
        for (w, c, side) in [
            (e.border_left_width, &e.border_left_color, "left"),
            (e.border_right_width, &e.border_right_color, "right"),
        ] {
            if w <= 1.0 {
                continue;
            }
            let Some(col) = color::parse(c) else { continue };
            if col.a < 0.2 || !col.has_chroma(40) {
                continue;
            }
            out.push(Finding {
                rule: "side-tab",
                severity: Severity::Major,
                location: e.location(),
                evidence: format!("border-{}: {:.0}px {}", side, w, col.hex()),
                remedy: "Drop the colored side stripe, or take it to 1px. It is the most recognizable tell of generated UI.",
            });
        }
    }
}

fn glow(els: &[Element], out: &mut Vec<Finding>) {
    for e in els {
        if let Some(hex) = is_glow(&e.box_shadow) {
            out.push(Finding {
                rule: "glow-shadow",
                severity: Severity::Major,
                location: e.location(),
                evidence: format!("zero-offset colored glow {}", hex),
                remedy: "Use a neutral shadow with real offset and blur; a light source has a direction.",
            });
        }
    }
}

fn nested_cards(els: &[Element], out: &mut Vec<Finding>) {
    // Index cards by their DFS index so we can test ancestry cheaply.
    let cards: Vec<&Element> = els.iter().filter(|e| is_card(e)).collect();
    for c in &cards {
        // A card whose ancestor chain contains another card, and which sits
        // visually inside it.
        for outer in &cards {
            if outer.index >= c.index {
                continue;
            }
            let inside = c.x >= outer.x - 1.0
                && c.y >= outer.y - 1.0
                && c.x + c.width <= outer.x + outer.width + 1.0
                && c.y + c.height <= outer.y + outer.height + 1.0;
            if inside && c.width < outer.width && c.height < outer.height {
                out.push(Finding {
                    rule: "nested-cards",
                    severity: Severity::Major,
                    location: c.location(),
                    evidence: format!(
                        "card {:.0}x{:.0} inside card {:.0}x{:.0}",
                        c.width, c.height, outer.width, outer.height
                    ),
                    remedy: "Flatten one level: give the inner content spacing instead of a second surface.",
                });
                break;
            }
        }
    }
}

fn gradient_text(els: &[Element], out: &mut Vec<Finding>) {
    for e in els {
        if !has_own_text(e) {
            continue;
        }
        let transparent = color::parse(&e.color).map(|c| c.a < 0.05).unwrap_or(false);
        if transparent && e.background_image.contains("gradient") {
            out.push(Finding {
                rule: "gradient-text",
                severity: Severity::Major,
                location: e.location(),
                evidence: "background-clip:text over a gradient".to_string(),
                remedy: "Set the text in a solid color; carry emphasis with weight or size.",
            });
        }
    }
}

fn kicker(els: &[Element], out: &mut Vec<Finding>) {
    let headings: Vec<&Element> = els
        .iter()
        .filter(|e| matches!(e.tag.as_str(), "h1" | "h2"))
        .collect();
    for h in headings {
        // A small, tracked or uppercase label sitting directly above a heading.
        for e in els {
            if e.index >= h.index || !has_own_text(e) {
                continue;
            }
            let vertical_gap = h.y - (e.y + e.height);
            if !(-2.0..=40.0).contains(&vertical_gap) {
                continue;
            }
            if e.font_size_px >= h.font_size_px || e.font_size_px > 18.0 {
                continue;
            }
            let tracked = e
                .letter_spacing
                .trim_end_matches("px")
                .parse::<f64>()
                .map(|v| v >= 1.0)
                .unwrap_or(false);
            let upper = e.text_transform == "uppercase"
                || (e.own_text.chars().filter(|c| c.is_alphabetic()).count() >= 3
                    && e.own_text
                        .chars()
                        .filter(|c| c.is_alphabetic())
                        .all(|c| c.is_uppercase()));
            if tracked || upper {
                out.push(Finding {
                    rule: "kicker-above-heading",
                    severity: Severity::Major,
                    location: e.location(),
                    evidence: format!(
                        "tracked/caps label above <{}> \"{}\"",
                        h.tag,
                        h.own_text.chars().take(24).collect::<String>()
                    ),
                    remedy: "Delete the eyebrow. The heading carries its own weight.",
                });
                break;
            }
        }
    }
}

fn typography(els: &[Element], out: &mut Vec<Finding>) {
    let mut families: HashMap<String, usize> = HashMap::new();
    for e in els {
        if !has_own_text(e) {
            continue;
        }
        let fam = first_family(&e.font_family);
        if fam.is_empty() || generic_family(&fam) {
            continue;
        }
        *families.entry(fam).or_insert(0) += 1;
    }
    // Default display faces: flag the largest text using one.
    let mut worst: Option<(&Element, String)> = None;
    for e in els {
        if !has_own_text(e) || e.font_size_px < 20.0 {
            continue;
        }
        let fam = first_family(&e.font_family);
        if DEFAULT_FACES.contains(&fam.as_str())
            && worst
                .as_ref()
                .map(|(w, _)| e.font_size_px > w.font_size_px)
                .unwrap_or(true)
        {
            worst = Some((e, fam));
        }
    }
    if let Some((e, fam)) = worst {
        out.push(Finding {
            rule: "default-display-face",
            // Taste, not defect: a project's brand face may legitimately be one
            // of these, so this informs rather than blocks.
            severity: Severity::Minor,
            location: e.location(),
            evidence: format!("display type set in {} at {:.0}px", fam, e.font_size_px),
            remedy: "Choose a display face with a point of view; these are the defaults every generated UI reaches for.",
        });
    }
}

fn heading_order(els: &[Element], out: &mut Vec<Finding>) {
    let mut last: Option<(u8, String)> = None;
    for e in els {
        let level = match e.tag.as_str() {
            "h1" => 1u8,
            "h2" => 2,
            "h3" => 3,
            "h4" => 4,
            "h5" => 5,
            "h6" => 6,
            _ => continue,
        };
        if let Some((prev, prev_text)) = &last {
            if level > prev + 1 {
                out.push(Finding {
                    rule: "skipped-heading",
                    severity: Severity::Minor,
                    location: e.location(),
                    evidence: format!(
                        "<h{}> \"{}\" follows <h{}> \"{}\"",
                        level,
                        e.own_text.chars().take(24).collect::<String>(),
                        prev,
                        prev_text.chars().take(24).collect::<String>()
                    ),
                    remedy:
                        "Use the next heading level down; screen readers navigate by this outline.",
                });
            }
        }
        last = Some((level, e.own_text.clone()));
    }
}

fn tiny_text(els: &[Element], out: &mut Vec<Finding>) {
    for e in els {
        if !has_own_text(e) || e.font_size_px <= 0.0 {
            continue;
        }
        if e.font_size_px < 11.0 && !matches!(e.tag.as_str(), "sup" | "sub") {
            out.push(Finding {
                rule: "undersized-text",
                severity: Severity::Minor,
                location: e.location(),
                evidence: format!("{:.0}px functional text", e.font_size_px),
                remedy: "Functional text below 11px fails on small and high-DPI screens; raise it.",
            });
        }
    }
}

fn measure(els: &[Element], out: &mut Vec<Finding>) {
    for e in els {
        if e.tag != "p" || !has_own_text(e) {
            continue;
        }
        if e.font_size_px <= 0.0 || e.width <= 0.0 {
            continue;
        }
        // Only real body copy has a measure problem. A short label that happens
        // to be a <p> spans its container but wraps nothing, and flagging it
        // would bury the finding that matters.
        let chars = e.own_text.trim().chars().count();
        if chars < 80 {
            continue;
        }
        // Approximate character width as 0.5em, the usual heuristic for
        // proportional Latin text.
        let ch = e.width / (e.font_size_px * 0.5);
        if ch > 90.0 {
            out.push(Finding {
                rule: "long-measure",
                severity: Severity::Minor,
                location: e.location(),
                evidence: format!("~{:.0}ch line length ({:.0}px wide)", ch, e.width),
                remedy: "Constrain body copy to roughly 65-75ch; long lines lose the reader between lines.",
            });
        }
    }
}

/// Placeholder copy. Checked in the browser engine too, since the rendered text
/// is the text a reader actually sees.
fn placeholder_copy(els: &[Element], out: &mut Vec<Finding>) {
    for e in els {
        if !has_own_text(e) {
            continue;
        }
        let t = e.own_text.to_ascii_lowercase();
        if t.contains("lorem ipsum") {
            out.push(Finding {
                rule: "placeholder-copy",
                severity: Severity::Minor,
                location: e.location(),
                evidence: "lorem ipsum in visible copy".to_string(),
                remedy: "Use real product content; placeholder copy hides real layout problems.",
            });
        }
    }
}

fn touch_targets(els: &[Element], out: &mut Vec<Finding>) {
    for e in els {
        let interactive = matches!(e.tag.as_str(), "button" | "a" | "input" | "select")
            || e.class.contains("btn")
            || e.class.contains("button");
        if !interactive || e.width <= 0.0 || e.height <= 0.0 {
            continue;
        }
        // Inline links inside prose are not tap targets in the same sense.
        if e.tag == "a" && e.height < 24.0 && e.parent_tag() == Some("p") {
            continue;
        }
        if e.height < 44.0 && has_own_text(e) {
            out.push(Finding {
                rule: "small-touch-target",
                severity: Severity::Minor,
                location: e.location(),
                evidence: format!("{:.0}x{:.0}px interactive target", e.width, e.height),
                remedy: "Give tap targets at least 44px of height.",
            });
        }
    }
}

fn overflow(els: &[Element], out: &mut Vec<Finding>) {
    for e in els {
        if e.client_width <= 0.0 {
            continue;
        }
        // Only flag clipped overflow: a scrollable region is intentional.
        let clipped = e.overflow.contains("hidden") || e.overflow.contains("clip");
        if clipped && e.scroll_width > e.client_width + 2.0 {
            out.push(Finding {
                rule: "clipped-overflow",
                severity: Severity::Minor,
                location: e.location(),
                evidence: format!(
                    "content {:.0}px wide clipped to {:.0}px",
                    e.scroll_width, e.client_width
                ),
                remedy: "Content is being cut off — let it wrap, truncate deliberately, or widen the container.",
            });
        }
    }
}

fn semantic_button(els: &[Element], out: &mut Vec<Finding>) {
    for e in els {
        if e.tag != "div" && e.tag != "span" {
            continue;
        }
        let looks_clickable = e.has_click
            || e.class.contains("btn")
            || e.class.contains("button")
            || e.cursor == "pointer";
        if looks_clickable && has_own_text(e) {
            out.push(Finding {
                rule: "non-semantic-control",
                severity: Severity::Minor,
                location: e.location(),
                evidence: format!("<{}> styled as a control", e.tag),
                remedy: "Use <button>; a div is not focusable or announced as a control.",
            });
        }
    }
}

fn decorative_blur(els: &[Element], out: &mut Vec<Finding>) {
    for e in els {
        if e.backdrop_filter == "none" || e.backdrop_filter.is_empty() {
            continue;
        }
        if e.backdrop_filter.contains("blur") {
            out.push(Finding {
                rule: "decorative-glass",
                severity: Severity::Advisory,
                location: e.location(),
                evidence: format!("backdrop-filter: {}", e.backdrop_filter),
                remedy:
                    "Keep glass/blur for surfaces that genuinely overlap content, not as texture.",
            });
        }
    }
}

fn token_drift(els: &[Element], t: &Tokens, out: &mut Vec<Finding>) {
    let mut seen_colors: Vec<String> = Vec::new();
    for e in els {
        if has_own_text(e) && !t.colors.is_empty() {
            if let Some(c) = color::parse(&e.color) {
                if c.a > 0.05 && !t.allows_color(c) && !seen_colors.contains(&c.hex()) {
                    seen_colors.push(c.hex());
                    out.push(Finding {
                        rule: "token-drift-color",
                        severity: Severity::Minor,
                        location: e.location(),
                        evidence: format!("{} is not in design-system.md", c.hex()),
                        remedy: "Use a declared palette role, or add this color to .formwork/design-system.md deliberately.",
                    });
                }
            }
        }
        if !t.fonts.is_empty() && has_own_text(e) {
            let fam = first_family(&e.font_family);
            if !fam.is_empty() && !generic_family(&fam) && !t.allows_font(&e.font_family) {
                out.push(Finding {
                    rule: "token-drift-font",
                    severity: Severity::Minor,
                    location: e.location(),
                    evidence: format!("{} is not in design-system.md", fam),
                    remedy:
                        "Use a declared family, or add it to .formwork/design-system.md deliberately.",
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_multi_layer_shadows() {
        let v = "rgba(0, 0, 0, 0.1) 0px 1px 2px 0px, rgb(139, 92, 246) 0px 0px 24px 0px";
        let layers = split_shadow_layers(v);
        assert_eq!(layers.len(), 2);
        assert!(layers[1].contains("139"));
    }

    #[test]
    fn detects_zero_offset_colored_glow() {
        assert!(is_glow("rgb(139, 92, 246) 0px 0px 24px 0px").is_some());
        // Real elevation: has offset.
        assert!(is_glow("rgba(0, 0, 0, 0.2) 0px 4px 12px 0px").is_none());
        // Neutral, even at zero offset, is not the "cool glow" pattern.
        assert!(is_glow("rgba(0, 0, 0, 0.3) 0px 0px 8px 0px").is_none());
        assert!(is_glow("none").is_none());
    }

    #[test]
    fn first_family_strips_quotes_and_fallbacks() {
        assert_eq!(
            first_family("\"Iowan Old Style\", Georgia, serif"),
            "iowan old style"
        );
        assert_eq!(first_family("Inter, sans-serif"), "inter");
        assert_eq!(first_family(""), "");
    }

    #[test]
    fn tokens_allow_neutrals_and_declared_colors() {
        let t = Tokens {
            colors: vec![color::parse("#b8422e").unwrap()],
            ..Default::default()
        };
        // Neutral grays never count as drift.
        assert!(t.allows_color(color::parse("#6b7280").unwrap()));
        // Declared, within tolerance.
        assert!(t.allows_color(color::parse("#b8432f").unwrap()));
        // Undeclared chromatic color.
        assert!(!t.allows_color(color::parse("#7c3aed").unwrap()));
    }

    #[test]
    fn empty_token_categories_abstain() {
        let t = Tokens::default();
        assert!(t.allows_font("Inter, sans-serif"));
        assert!(!t.has_any());
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
            xs[(self.next() as usize) % xs.len()]
        }
        fn num(&mut self) -> String {
            // Values chosen to poke rule thresholds and edge geometry.
            let choices = [
                "0",
                "1",
                "1.5",
                "4",
                "11",
                "24",
                "44",
                "-5",
                "1e9",
                "-1e9",
                "NaN",
                "inf",
                "0.0000001",
                "999999",
            ];
            choices[(self.next() as usize) % choices.len()].to_string()
        }
    }

    /// The rule engine runs on data derived from a browser. It must never panic
    /// regardless of what the page produced: malformed colors, absurd geometry,
    /// non-finite sizes, multi-byte text, or an ancestor chain that disagrees
    /// with the layout. Built via the real JSON path so this also covers
    /// `elements_from_json`.
    #[test]
    fn rules_never_panic_on_adversarial_elements() {
        use crate::design_lint::cdp::elements_from_json;

        let mut rng = Rng(0xC3D2E1F0A9B8C7D6);
        let tags = [
            "div", "span", "p", "h1", "h2", "h3", "button", "a", "section", "li", "",
        ];
        let colors = [
            "#8b5cf6",
            "rgb(1,2,3)",
            "rgba(0,0,0,0)",
            "rgba(0,0,0,NaN)",
            "transparent",
            "oklch(60% .2 350)",
            "not-a-color",
            "",
            "#fff",
            "日本語",
            "rgb(inf,0,0)",
        ];
        let shadows = [
            "none",
            "",
            "rgb(139,92,246) 0px 0px 24px 0px",
            "rgba(0,0,0,.2) 0 4px 12px",
            ") rgb( 0 0",
            "garbage 1 2",
        ];
        let texts = [
            "",
            "hello",
            "Lorem ipsum dolor",
            "\u{2014}\u{201c}\u{201d}",
            "\u{65e5}\u{672c}\u{8a9e}",
            "INTRODUCING",
        ];
        let radii = ["0px", "12px", "50%", "9999px", "", "-4px", "NaNpx"];
        let families = [
            "Inter, sans-serif",
            "\"Iowan Old Style\", serif",
            "",
            "system-ui",
            "\u{65e5}",
        ];

        let tokens = Tokens {
            colors: vec![color::parse("#b8422e").unwrap()],
            fonts: vec!["georgia".to_string()],
            radii: vec!["8px".to_string()],
        };

        for _ in 0..6_000 {
            let n = 1 + (rng.next() as usize % 6);
            let mut rows = Vec::new();
            for _ in 0..n {
                rows.push(serde_json::json!({
                    "tag": rng.pick(&tags),
                    "class": rng.pick(&["card", "btn rounded", "", "wrap"]),
                    "id": "",
                    "index": (rng.next() % 50),
                    "ancestors": ["html", "body", rng.pick(&tags)],
                    "text": rng.pick(&texts),
                    "ownText": rng.pick(&texts),
                    "color": rng.pick(&colors),
                    "background": rng.pick(&colors),
                    "effectiveBg": rng.pick(&colors),
                    "fontFamily": rng.pick(&families),
                    "fontSize": rng.num(),
                    "fontWeight": rng.num(),
                    "letterSpacing": rng.num(),
                    "lineHeight": rng.num(),
                    "textTransform": rng.pick(&["none", "uppercase"]),
                    "textAlign": rng.pick(&["left", "justify"]),
                    "borderLeftWidth": rng.num(),
                    "borderLeftColor": rng.pick(&colors),
                    "borderRightWidth": rng.num(),
                    "borderRightColor": rng.pick(&colors),
                    "borderRadius": rng.pick(&radii),
                    "boxShadow": rng.pick(&shadows),
                    "backgroundImage": rng.pick(&["none", "linear-gradient(#fff,#000)", "url(x)"]),
                    "backdropFilter": rng.pick(&["none", "blur(12px)"]),
                    "overflow": rng.pick(&["visible", "hidden", "clip"]),
                    "whiteSpace": "normal",
                    "position": "static",
                    "width": rng.num(),
                    "height": rng.num(),
                    "x": rng.num(),
                    "y": rng.num(),
                    "scrollWidth": rng.num(),
                    "scrollHeight": rng.num(),
                    "clientWidth": rng.num(),
                    "clientHeight": rng.num(),
                    "hasClick": (rng.next() & 1) == 0,
                    "cursor": rng.pick(&["auto", "pointer"]),
                    "bgUncertain": (rng.next() & 1) == 0,
                    "srOnly": (rng.next() & 1) == 0,
                    "opacity": rng.num(),
                }));
            }
            let raw = serde_json::Value::Array(rows).to_string();
            let els = elements_from_json(&raw).unwrap();
            // Both with and without tokens.
            let _ = run(&els, None);
            let _ = run(&els, Some(&tokens));
        }
    }
}
