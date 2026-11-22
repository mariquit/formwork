// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>

//! Color parsing and WCAG contrast math.
//!
//! The contrast implementation is checked against known reference values in
//! the tests below, including the four cases measured against the prior-art
//! JS engine during design (1.9 / 3.8 / 4.1) and the absolute 21:1 reference.

/// An opaque or translucent sRGB color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f64,
}

impl Rgba {
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Composite this color over an opaque backdrop. Needed because computed
    /// backgrounds are frequently `rgba(...)` over an ancestor, and comparing
    /// contrast against a translucent value would be meaningless.
    pub fn over(self, bg: Rgba) -> Rgba {
        if self.a >= 1.0 {
            return self;
        }
        let mix = |f: u8, b: u8| ((f as f64) * self.a + (b as f64) * (1.0 - self.a)).round() as u8;
        Rgba {
            r: mix(self.r, bg.r),
            g: mix(self.g, bg.g),
            b: mix(self.b, bg.b),
            a: 1.0,
        }
    }

    /// True when the color carries meaningful saturation. `tol` is the
    /// max-min channel spread; higher means "more clearly chromatic".
    pub fn has_chroma(self, tol: u8) -> bool {
        let max = self.r.max(self.g).max(self.b);
        let min = self.r.min(self.g).min(self.b);
        max.saturating_sub(min) > tol
    }
}

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.03928 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG 2.x relative luminance.
pub fn relative_luminance(c: Rgba) -> f64 {
    0.2126 * srgb_to_linear(c.r as f64 / 255.0)
        + 0.7152 * srgb_to_linear(c.g as f64 / 255.0)
        + 0.0722 * srgb_to_linear(c.b as f64 / 255.0)
}

/// WCAG 2.x contrast ratio, always >= 1.0.
pub fn contrast_ratio(a: Rgba, b: Rgba) -> f64 {
    let (l1, l2) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

/// WCAG "large text" threshold: >=24px, or >=18.66px when bold. Large text
/// only needs 3:1 rather than 4.5:1.
pub fn is_large_text(font_px: f64, weight: u16) -> bool {
    font_px >= 24.0 || (font_px >= 18.66 && weight >= 700)
}

fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Parse a CSS color. Handles `#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb()`,
/// `rgba()` (comma or space separated), `transparent`, and the named colors
/// that actually show up in generated UI. Returns None for anything else
/// (gradients, `color-mix()`, `oklch()`) — abstaining is correct, since a
/// wrong guess produces a false finding.
pub fn parse(input: &str) -> Option<Rgba> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();

    if lower == "transparent" {
        return Some(Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 0.0,
        });
    }

    if let Some(hex) = lower.strip_prefix('#') {
        let b = hex.as_bytes();
        let n = |i: usize| hex_nibble(b[i]);
        return match b.len() {
            3 => Some(Rgba {
                r: n(0)? * 17,
                g: n(1)? * 17,
                b: n(2)? * 17,
                a: 1.0,
            }),
            6 => Some(Rgba {
                r: n(0)? * 16 + n(1)?,
                g: n(2)? * 16 + n(3)?,
                b: n(4)? * 16 + n(5)?,
                a: 1.0,
            }),
            8 => Some(Rgba {
                r: n(0)? * 16 + n(1)?,
                g: n(2)? * 16 + n(3)?,
                b: n(4)? * 16 + n(5)?,
                a: (n(6)? * 16 + n(7)?) as f64 / 255.0,
            }),
            _ => None,
        };
    }

    if let Some(rest) = lower
        .strip_prefix("rgba(")
        .or_else(|| lower.strip_prefix("rgb("))
    {
        let body = rest.strip_suffix(')')?;
        // Accept "r, g, b", "r g b", and "r g b / a".
        let body = body.replace('/', " ");
        let parts: Vec<&str> = body
            .split([',', ' '])
            .filter(|p| !p.trim().is_empty())
            .collect();
        if parts.len() < 3 {
            return None;
        }
        let chan = |t: &str| -> Option<u8> {
            let t = t.trim();
            if let Some(pct) = t.strip_suffix('%') {
                let v: f64 = pct.parse().ok()?;
                if !v.is_finite() {
                    return None;
                }
                Some((v / 100.0 * 255.0).round().clamp(0.0, 255.0) as u8)
            } else {
                let v: f64 = t.parse().ok()?;
                if !v.is_finite() {
                    return None;
                }
                Some(v.round().clamp(0.0, 255.0) as u8)
            }
        };
        let a = if parts.len() >= 4 {
            let t = parts[3].trim();
            let raw = if let Some(pct) = t.strip_suffix('%') {
                pct.parse::<f64>().ok()? / 100.0
            } else {
                t.parse::<f64>().ok()?
            };
            // Reject NaN/inf: a non-finite alpha would poison `over()` and make
            // `a >= 1.0` behave unpredictably.
            if !raw.is_finite() {
                return None;
            }
            raw
        } else {
            1.0
        };
        return Some(Rgba {
            r: chan(parts[0])?,
            g: chan(parts[1])?,
            b: chan(parts[2])?,
            a: a.clamp(0.0, 1.0),
        });
    }

    named(&lower)
}

fn named(name: &str) -> Option<Rgba> {
    let opaque = |r, g, b| Some(Rgba { r, g, b, a: 1.0 });
    match name {
        "white" => opaque(255, 255, 255),
        "black" => opaque(0, 0, 0),
        "red" => opaque(255, 0, 0),
        "green" => opaque(0, 128, 0),
        "blue" => opaque(0, 0, 255),
        "gray" | "grey" => opaque(128, 128, 128),
        "silver" => opaque(192, 192, 192),
        "purple" => opaque(128, 0, 128),
        "violet" => opaque(238, 130, 238),
        "indigo" => opaque(75, 0, 130),
        "cyan" | "aqua" => opaque(0, 255, 255),
        "teal" => opaque(0, 128, 128),
        "orange" => opaque(255, 165, 0),
        "yellow" => opaque(255, 255, 0),
        "pink" => opaque(255, 192, 203),
        "brown" => opaque(165, 42, 42),
        "navy" => opaque(0, 0, 128),
        "beige" => opaque(245, 245, 220),
        "ivory" => opaque(255, 255, 240),
        "cream" => opaque(255, 253, 208),
        "whitesmoke" => opaque(245, 245, 245),
        "slategray" | "slategrey" => opaque(112, 128, 144),
        _ => None,
    }
}

/// Every `#rrggbb`/`#rgb`/`#rrggbbaa` literal in a string.
///
/// Scans by char so multi-byte text (em-dashes in prose, non-Latin copy) can
/// never produce a non-char-boundary slice panic.
pub fn hex_literals(text: &str) -> Vec<Rgba> {
    let mut out = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] != '#' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        let mut hex = String::new();
        while j < chars.len() && chars[j].is_ascii_hexdigit() && hex.len() < 8 {
            hex.push(chars[j]);
            j += 1;
        }
        // Only exact CSS hex lengths count; 4/5/7 digits are not colors.
        if matches!(hex.len(), 3 | 6 | 8) {
            if let Some(c) = parse(&format!("#{}", hex)) {
                out.push(c);
            }
        }
        i = j.max(i + 1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Rgba {
        parse(s).unwrap()
    }

    /// The four values measured against the prior-art JS engine during design.
    /// If this test drifts, our contrast math no longer agrees with a known
    /// reference implementation.
    #[test]
    fn contrast_matches_reference_values() {
        let r = |a: &str, b: &str| (contrast_ratio(hex(a), hex(b)) * 10.0).round() / 10.0;
        assert_eq!(r("#b8bec7", "#ffffff"), 1.9);
        assert_eq!(r("#6b7280", "#141420"), 3.8);
        assert_eq!(r("#6b7280", "#0b0b12"), 4.1);
        // Absolute WCAG reference: pure black on pure white.
        assert_eq!(r("#000000", "#ffffff"), 21.0);
        // Identical colors are always 1:1.
        assert_eq!(r("#7c3aed", "#7c3aed"), 1.0);
    }

    #[test]
    fn contrast_is_symmetric() {
        let a = hex("#123456");
        let b = hex("#fedcba");
        assert!((contrast_ratio(a, b) - contrast_ratio(b, a)).abs() < 1e-12);
    }

    #[test]
    fn parses_css_color_forms() {
        assert_eq!(hex("#fff"), hex("#ffffff"));
        assert_eq!(hex("rgb(255, 255, 255)"), hex("#ffffff"));
        assert_eq!(hex("rgb(255 255 255)"), hex("#ffffff"));
        assert_eq!(hex("  #8B5CF6 "), hex("#8b5cf6"));
        assert_eq!(parse("rgba(0, 0, 0, 0.5)").unwrap().a, 0.5);
        assert_eq!(parse("rgb(0 0 0 / 50%)").unwrap().a, 0.5);
        assert_eq!(parse("transparent").unwrap().a, 0.0);
        assert_eq!(hex("white"), hex("#ffffff"));
    }

    /// Abstaining beats guessing: an unparseable color must not silently
    /// become black, which would invent contrast findings.
    #[test]
    fn abstains_on_unsupported_syntax() {
        assert!(parse("oklch(60% 0.25 350)").is_none());
        assert!(parse("linear-gradient(90deg, #fff, #000)").is_none());
        assert!(parse("color-mix(in srgb, red, blue)").is_none());
        assert!(parse("").is_none());
        assert!(parse("#12345").is_none());
        assert!(parse("notacolor").is_none());
    }

    /// Regression: a byte-indexed scanner panics on multi-byte input. This
    /// exact shape (an em-dash in prose next to a hex color) crashed the
    /// static engine.
    #[test]
    fn hex_scan_survives_multibyte_text() {
        let found = hex_literals("accent #7a2e1e — paper #faf7f2, ink #1f1d1a");
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].hex(), "#7a2e1e");
        assert_eq!(found[2].hex(), "#1f1d1a");
        // Non-color hash tokens must not be misread.
        assert!(hex_literals("#12345 #xyz #").is_empty());
    }

    #[test]
    fn alpha_composites_over_backdrop() {
        let half_black = parse("rgba(0,0,0,0.5)").unwrap();
        let white = hex("#ffffff");
        let out = half_black.over(white);
        assert_eq!(out.a, 1.0);
        // 50% black over white lands on mid gray.
        assert!((out.r as i16 - 128).abs() <= 1);
    }

    #[test]
    fn large_text_thresholds_follow_wcag() {
        assert!(is_large_text(24.0, 400));
        assert!(!is_large_text(23.0, 400));
        assert!(is_large_text(19.0, 700));
        assert!(!is_large_text(19.0, 400));
    }

    #[test]
    fn chroma_classifies_neutrals() {
        assert!(!hex("#808080").has_chroma(20));
        assert!(hex("#7c3aed").has_chroma(20));
    }

    /// A tiny deterministic xorshift PRNG. Reproducible: a failing seed can be
    /// pasted into a focused test. No external crate.
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

    /// `parse` must never panic, on any input. It returns `Option`, so its only
    /// failure mode is a crash — which is exactly what byte-indexing bugs cause.
    #[test]
    fn parse_never_panics_on_random_input() {
        let mut rng = Rng(0x9E3779B97F4A7C15);
        // Alphabet skewed toward the characters that drive the parser's
        // branches, plus multi-byte characters that break byte-indexed code.
        let alpha: Vec<char> = "#0123456789abcdefABCDEF(),/ %.rgbatransparen"
            .chars()
            .collect();
        let multibyte = ['\u{2014}', '\u{201c}', '\u{00e9}', '\u{65e5}', '\u{1F600}'];
        for _ in 0..20_000 {
            let len = (rng.byte() % 24) as usize;
            let mut s = String::new();
            for _ in 0..len {
                if rng.byte() < 32 {
                    s.push(multibyte[(rng.byte() as usize) % multibyte.len()]);
                } else {
                    s.push(alpha[(rng.byte() as usize) % alpha.len()]);
                }
            }
            if let Some(c) = parse(&s) {
                assert!(c.a.is_finite(), "non-finite alpha from {s:?}");
            }
            let _ = hex_literals(&s);
        }
    }

    /// Specific adversarial shapes that historically break color parsers.
    #[test]
    fn parse_handles_known_adversarial_shapes() {
        let cases = [
            "#",
            "#g",
            "#12",
            "#1234",    // 4 hex: not a CSS length
            "#1234567", // 7 hex
            "#ééé",     // multi-byte after the hash
            "rgb(",
            "rgb()",
            "rgb(,,)",
            "rgb(1)",
            "rgb(NaN, 0, 0)",
            "rgb(inf, 0, 0)",
            "rgba(0,0,0,NaN)",
            "rgba(0,0,0,inf)",
            "rgb(999999999999999999999, 0, 0)",
            "rgb(-5, -5, -5)",
            "rgb(1 2 3 / )",
            "rgba(1,2,3,4,5,6)",
            "  RGB( 1 , 2 , 3 )  ",
            "rgb(1%,2%,3%)",
            "transparent",
            "日本語",
        ];
        for input in cases {
            // Only assertion: does not panic, and any color it returns is sane.
            if let Some(c) = parse(input) {
                assert!(c.a.is_finite(), "non-finite alpha from {input:?}");
                assert!(
                    (0.0..=1.0).contains(&c.a),
                    "alpha out of range from {input:?}"
                );
            }
        }
    }

    /// `hex_literals` scans arbitrary prose; it must survive any string,
    /// including invalid-UTF-8-turned-replacement-char sequences.
    #[test]
    fn hex_literals_never_panics() {
        let mut rng = Rng(0xD1B54A32D192ED03);
        for _ in 0..10_000 {
            let len = (rng.byte() % 40) as usize;
            let bytes: Vec<u8> = (0..len).map(|_| rng.byte()).collect();
            let s = String::from_utf8_lossy(&bytes);
            let _ = hex_literals(&s);
        }
    }
}
