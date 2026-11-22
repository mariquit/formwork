// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>

//! Static fallback engine.
//!
//! Runs when no browser is available. This is a genuinely weaker check and
//! must never be presented otherwise: it reads the HTML as text, so it cannot
//! resolve the cascade and it cannot see through Tailwind utility classes. A
//! `border-l-4 border-violet-500` card is invisible to everything here except
//! the class-name heuristics below.
//!
//! Measured during design on a Tailwind mockup: this class of engine found 4
//! findings where the browser engine found 13.
//!
//! Rules here are therefore conservative — they fire only on patterns that are
//! unambiguous in source text, and several are Advisory so they can never
//! block a design on a machine that merely lacks Chrome.

use super::rules::{Tokens, DEFAULT_FACES};
use super::{Finding, Severity};

/// Strip HTML tags for prose-level checks.
///
/// Iterates by char, not by byte: `ui.html` routinely contains em-dashes and
/// other multi-byte text, and slicing a `str` at a byte offset inside a
/// character panics.
fn visible_text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut skipping = false;

    // Char-indexed view so tag lookahead is always on a char boundary.
    let chars: Vec<(usize, char)> = html.char_indices().collect();
    let lower = html.to_ascii_lowercase();

    for (n, &(byte_idx, ch)) in chars.iter().enumerate() {
        let rest = &lower[byte_idx..];
        if rest.starts_with("<script") || rest.starts_with("<style") {
            skipping = true;
        } else if rest.starts_with("</script") || rest.starts_with("</style") {
            skipping = false;
        }
        let _ = n;
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag && !skipping => out.push(c),
            _ => {}
        }
    }
    out
}

/// The contents of every `<style>` block, lowercased.
fn style_blocks(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut css = String::new();
    let mut from = 0usize;
    while let Some(start) = lower[from..].find("<style") {
        let abs = from + start;
        let Some(open_end) = lower[abs..].find('>') else {
            break;
        };
        let body_start = abs + open_end + 1;
        let Some(close) = lower[body_start..].find("</style") else {
            break;
        };
        css.push_str(&lower[body_start..body_start + close]);
        css.push('\n');
        from = body_start + close;
    }
    css
}

fn line_of(html: &str, needle: &str) -> String {
    for (n, line) in html.lines().enumerate() {
        if line.to_ascii_lowercase().contains(needle) {
            return format!("line {}", n + 1);
        }
    }
    "source".to_string()
}

pub fn lint(html: &str, tokens: Option<&Tokens>) -> Vec<Finding> {
    let mut out = Vec::new();
    let css = style_blocks(html);
    let lower = html.to_ascii_lowercase();
    let text = visible_text(html);

    // --- Patterns visible in raw CSS -------------------------------------

    // Colored side stripe, declared literally.
    for prop in ["border-left:", "border-right:"] {
        if let Some(p) = css.find(prop) {
            let decl: String = css[p..]
                .chars()
                .take(80)
                .take_while(|&c| c != ';')
                .collect();
            let px = decl
                .split_whitespace()
                .find_map(|t| t.trim_end_matches("px").parse::<f64>().ok())
                .unwrap_or(0.0);
            let chromatic = super::color::parse(
                decl.split_whitespace()
                    .last()
                    .unwrap_or("")
                    .trim_end_matches(';'),
            )
            .map(|c| c.has_chroma(40))
            .unwrap_or(false);
            if px > 1.0 && chromatic {
                out.push(Finding {
                    rule: "side-tab",
                    severity: Severity::Major,
                    location: line_of(html, prop),
                    evidence: decl.trim().to_string(),
                    remedy: "Drop the colored side stripe, or take it to 1px.",
                });
            }
        }
    }

    // Tailwind utility form of the same thing.
    if lower.contains("border-l-4") || lower.contains("border-l-8") {
        out.push(Finding {
            rule: "side-tab",
            severity: Severity::Major,
            location: line_of(html, "border-l-"),
            evidence: "Tailwind border-l-4/8 utility".to_string(),
            remedy: "Drop the colored side stripe, or take it to border-l (1px).",
        });
    }

    // Gradient text, both forms.
    if (css.contains("background-clip") && css.contains("text") && css.contains("gradient"))
        || (lower.contains("bg-clip-text") && lower.contains("bg-gradient"))
    {
        out.push(Finding {
            rule: "gradient-text",
            severity: Severity::Major,
            location: line_of(html, "clip"),
            evidence: "background-clip:text over a gradient".to_string(),
            remedy: "Set the text in a solid color; carry emphasis with weight or size.",
        });
    }

    // Zero-offset colored glow declared in CSS.
    for decl in css.split(';') {
        if !decl.contains("box-shadow") {
            continue;
        }
        let has_zero_offset = decl.contains("0 0 ") || decl.contains("0px 0px");
        let chromatic = decl.contains("rgba(") || decl.contains('#');
        if has_zero_offset && chromatic {
            if let Some(c) = decl
                .split_whitespace()
                .find(|t| t.starts_with('#'))
                .and_then(super::color::parse)
            {
                if c.has_chroma(40) {
                    out.push(Finding {
                        rule: "glow-shadow",
                        severity: Severity::Major,
                        location: line_of(html, "box-shadow"),
                        evidence: format!("zero-offset colored glow {}", c.hex()),
                        remedy: "Use a neutral shadow with real offset and blur.",
                    });
                }
            }
        }
    }

    // Default display faces.
    for face in DEFAULT_FACES {
        if css.contains(&format!("font-family:{}", face))
            || css.contains(&format!("font-family: {}", face))
            || css.contains(&format!("font-family:\"{}\"", face))
            || css.contains(&format!("font-family: \"{}\"", face))
        {
            out.push(Finding {
                rule: "default-display-face",
                // Taste, not defect — see Severity docs. Must match the CDP
                // engine's severity so a finding does not change weight based
                // on whether a browser happened to be installed.
                severity: Severity::Minor,
                location: line_of(html, "font-family"),
                evidence: format!("font-family: {}", face),
                remedy: "Choose a display face with a point of view.",
            });
            break;
        }
    }

    if css.contains("backdrop-filter") && css.contains("blur") {
        out.push(Finding {
            rule: "decorative-glass",
            severity: Severity::Advisory,
            location: line_of(html, "backdrop-filter"),
            evidence: "backdrop-filter: blur(...)".to_string(),
            remedy: "Keep glass/blur for surfaces that genuinely overlap content.",
        });
    }

    // --- Patterns visible in markup --------------------------------------

    heading_order(html, &mut out);

    // Nested cards, via repeated card-ish class names on nested divs. Only
    // fires on the literal `card` token, which is unambiguous enough in source.
    if let Some(f) = nested_card_classes(html) {
        out.push(f);
    }

    // Lorem ipsum placeholder copy.
    let tl = text.to_ascii_lowercase();
    if tl.contains("lorem ipsum") {
        out.push(Finding {
            rule: "placeholder-copy",
            severity: Severity::Minor,
            location: line_of(html, "lorem ipsum"),
            evidence: "lorem ipsum in visible copy".to_string(),
            remedy: "Use real product content; placeholder copy hides real layout problems.",
        });
    }

    // Clickable div/span.
    if lower.contains("<div onclick") || lower.contains("<span onclick") {
        out.push(Finding {
            rule: "non-semantic-control",
            severity: Severity::Minor,
            location: line_of(html, "onclick"),
            evidence: "onclick on a <div>/<span>".to_string(),
            remedy: "Use <button>; a div is not focusable or announced as a control.",
        });
    }

    if let Some(t) = tokens {
        token_drift(&css, t, html, &mut out);
    }

    out
}

fn heading_order(html: &str, out: &mut Vec<Finding>) {
    let lower = html.to_ascii_lowercase();
    let mut seq: Vec<(u8, usize)> = Vec::new();
    for (n, line) in lower.lines().enumerate() {
        for lvl in 1u8..=6 {
            let tag = format!("<h{}", lvl);
            if line.contains(&tag) {
                seq.push((lvl, n + 1));
            }
        }
    }
    seq.sort_by_key(|(_, n)| *n);
    for w in seq.windows(2) {
        let (prev, _) = w[0];
        let (next, line) = w[1];
        if next > prev + 1 {
            out.push(Finding {
                rule: "skipped-heading",
                severity: Severity::Minor,
                location: format!("line {}", line),
                evidence: format!("<h{}> follows <h{}>", next, prev),
                remedy: "Use the next heading level down; screen readers navigate by this outline.",
            });
            break;
        }
    }
}

/// Detect a card-classed element nested inside another card-classed element,
/// by tracking div depth in source order.
fn nested_card_classes(html: &str) -> Option<Finding> {
    let lower = html.to_ascii_lowercase();
    let mut depth_stack: Vec<bool> = Vec::new();
    // Char boundaries only: slicing at a raw byte offset panics on multi-byte
    // text, which `ui.html` routinely contains.
    for (i, ch) in lower.char_indices() {
        if ch == '<' {
            let rest = &lower[i..];
            if rest.starts_with("</div") {
                depth_stack.pop();
            } else if rest.starts_with("<div") {
                let end = rest.find('>').unwrap_or(0);
                let tag = &rest[..end];
                let is_card = tag.contains("card")
                    || (tag.contains("rounded") && (tag.contains("bg-") || tag.contains("shadow")));
                if is_card && depth_stack.iter().any(|&c| c) {
                    let line = lower[..i].matches('\n').count() + 1;
                    return Some(Finding {
                        rule: "nested-cards",
                        severity: Severity::Major,
                        location: format!("line {}", line),
                        evidence: "card-classed element inside another card".to_string(),
                        remedy: "Flatten one level: give the inner content spacing instead of a second surface.",
                    });
                }
                if !tag.ends_with('/') {
                    depth_stack.push(is_card);
                }
            }
        }
    }
    None
}

fn token_drift(css: &str, t: &Tokens, html: &str, out: &mut Vec<Finding>) {
    if t.colors.is_empty() {
        return;
    }
    let mut reported: Vec<String> = Vec::new();
    for c in super::color::hex_literals(css) {
        if !c.has_chroma(24) || t.allows_color(c) || reported.contains(&c.hex()) {
            continue;
        }
        reported.push(c.hex());
        out.push(Finding {
            rule: "token-drift-color",
            severity: Severity::Minor,
            location: line_of(html, &c.hex()),
            evidence: format!("{} is not in design-system.md", c.hex()),
            remedy: "Use a declared palette role, or add this color to .formwork/design-system.md deliberately.",
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tags_and_script_bodies() {
        let t = visible_text("<p>hello</p><script>var x='no';</script><b>world</b>");
        assert!(t.contains("hello"));
        assert!(t.contains("world"));
        assert!(!t.contains("var x"));
    }

    #[test]
    fn extracts_style_blocks() {
        let css = style_blocks("<style>.a{color:RED}</style><p>x</p><style>.b{top:0}</style>");
        assert!(css.contains(".a{color:red}"));
        assert!(css.contains(".b{top:0}"));
    }

    #[test]
    fn flags_tailwind_side_tab_without_a_browser() {
        let f = lint(
            r#"<div class="rounded-xl border-l-4 border-violet-500">x</div>"#,
            None,
        );
        assert!(f.iter().any(|x| x.rule == "side-tab"));
    }

    #[test]
    fn flags_nested_cards_in_source() {
        let f = lint(
            r#"<div class="card"><div class="card"><p>x</p></div></div>"#,
            None,
        );
        assert!(f.iter().any(|x| x.rule == "nested-cards"));
    }

    #[test]
    fn clean_markup_has_no_blocking_findings() {
        let html = r#"<!DOCTYPE html><html><head><style>
            .p{background:#ffffff;color:#1a1a1a;font-family:Georgia,serif;font-size:16px}
            h1{font-size:34px}h2{font-size:22px}
            </style></head><body><div class="p"><h1>Ledger</h1><h2>Reconcile</h2>
            <p>Real copy about reconciling a print run.</p>
            <button>Reconcile run</button></div></body></html>"#;
        let f = lint(html, None);
        let blocking: Vec<&Finding> = f.iter().filter(|x| x.severity.blocks()).collect();
        assert!(
            blocking.is_empty(),
            "unexpected blocking findings: {:?}",
            blocking
        );
    }

    /// Regression: the static engine panicked on any multi-byte character
    /// (`start byte index N is not a char boundary`). Real mockups contain
    /// em-dashes, curly quotes, and non-Latin copy, so this must never panic.
    #[test]
    fn survives_multibyte_content() {
        let html = "<!DOCTYPE html><html><head><style>\
            body{color:#1f1d1a;background:#faf7f2}\
            </style></head><body>\
            <h1>Reconcile \u{2014} print runs</h1>\
            <p>Run 4471 \u{2014} 2,000 ordered, \u{201c}1,988\u{201d} delivered \u{2192} variance</p>\
            <p>\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30c6}\u{30ad}\u{30b9}\u{30c8}</p>\
            <div class=\"card\"><div class=\"card\">nested \u{2014} here</div></div>\
            </body></html>";
        let f = lint(html, None);
        assert!(f.iter().any(|x| x.rule == "nested-cards"));
        assert!(visible_text(html).contains("Reconcile"));
    }

    #[test]
    fn placeholder_copy_is_detected() {
        let f = lint("<p>Lorem ipsum dolor sit amet.</p>", None);
        assert!(f.iter().any(|x| x.rule == "placeholder-copy"));
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
        fn byte(&mut self) -> u8 {
            (self.next() & 0xff) as u8
        }
    }

    /// Every static-engine entry point must survive arbitrary input without
    /// panicking. `lint` is the whole pipeline; the two scanners are called
    /// directly because they do the byte-adjacent work.
    #[test]
    fn static_engine_never_panics_on_random_input() {
        let mut rng = Rng(0x243F6A8885A308D3);
        // Fragments that steer the parser's branches: tags, the exact tokens
        // rules key on, entities, and multi-byte text.
        let frags = [
            "<",
            ">",
            "</",
            "<div",
            "<style",
            "</style>",
            "<script",
            "</script>",
            "class=\"card\"",
            "border-l-4",
            "bg-clip-text",
            "bg-gradient",
            "box-shadow",
            "0 0 24px",
            "#8b5cf6",
            "rgba(",
            "font-family:inter",
            "backdrop-filter",
            "blur",
            "lorem ipsum",
            "onclick",
            "h1",
            "h3",
            "\u{2014}",
            "\u{1F600}",
            "日本語",
            "\"",
            "'",
            "/",
            "(",
            ")",
            ";",
            "{",
            "}",
            " ",
        ];
        for _ in 0..30_000 {
            let n = (rng.byte() % 24) as usize;
            let mut s = String::new();
            for _ in 0..n {
                s.push_str(frags[(rng.byte() as usize) % frags.len()]);
            }
            // None of these may panic on any assembled string.
            let _ = visible_text(&s);
            let _ = style_blocks(&s);
            let _ = lint(&s, None);
        }
    }

    /// Same, but with completely unstructured bytes (including invalid UTF-8
    /// coerced to replacement chars), which exercises the scanners' boundary
    /// handling more harshly than fragment assembly.
    #[test]
    fn static_engine_never_panics_on_raw_bytes() {
        let mut rng = Rng(0x452821E638D01377);
        for _ in 0..10_000 {
            let n = (rng.byte() % 64) as usize;
            let bytes: Vec<u8> = (0..n).map(|_| rng.byte()).collect();
            let s = String::from_utf8_lossy(&bytes);
            let _ = lint(&s, None);
        }
    }

    /// The specific class of input that already caused one shipped panic:
    /// multi-byte characters adjacent to the tokens rules scan for.
    #[test]
    fn static_engine_survives_multibyte_adjacent_to_tokens() {
        for html in [
            "<style>.a{color:#7a2e1e}\u{2014}</style><p>x</p>",
            "<div class=\"card\">\u{2014}<div class=\"card\">\u{201c}y\u{201d}</div></div>",
            "<h1>\u{65e5}\u{672c}\u{8a9e}</h1><h3>\u{2192}</h3>",
            "<style>font-family:\u{2014}inter</style>",
            "\u{1F600}<style>box-shadow:0 0 8px #8b5cf6\u{2014}</style>",
            "border-l-4\u{2014}\u{201c}\u{201d}",
        ] {
            let _ = lint(html, None);
        }
    }
}
