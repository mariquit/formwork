// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 John Anthony Mariquit <john@mariquit.com>

//! Chrome DevTools Protocol client.
//!
//! Launches a headless browser in its own process, navigates to the file, and
//! evaluates one JS probe that returns computed styles plus layout geometry
//! for every element. No browser engine is linked into this binary — the whole
//! cascade (including Tailwind utility classes) is resolved by Chrome.
//!
//! Everything here degrades rather than fails: any error returns a human
//! reason string that the caller records on the report, because a design gate
//! that hard-fails on a machine without Chrome would be unshippable.

use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// One element's resolved style and geometry, as read from the live page.
///
/// The page-side expression collects a wider slice of computed style than the
/// current rules consume, so some fields are captured but unread: `text`,
/// `line_height`, `text_transform`, `text_align`, `white_space`, `position`,
/// `scroll_height`, `client_width`, `client_height`. They are kept deliberately
/// — they are the raw material for typography and overflow rules, and dropping
/// one means editing the JS expression, the extraction, and this struct in
/// lockstep. Delete a field only when its rule is ruled out, not merely absent.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Element {
    pub tag: String,
    pub class: String,
    pub id: String,
    /// Depth-first index, used to build a stable location string.
    pub index: usize,
    /// Ancestor chain of tag names, outermost first.
    pub ancestors: Vec<String>,
    pub text: String,
    /// Direct text content only (excludes descendants' text).
    pub own_text: String,

    pub color: String,
    pub background: String,
    /// Nearest opaque ancestor background, already resolved by walking up in
    /// the page. This is what contrast must be measured against.
    pub effective_bg: String,
    pub font_family: String,
    pub font_size_px: f64,
    pub font_weight: u16,
    pub letter_spacing: String,
    pub line_height: String,
    pub text_transform: String,
    pub text_align: String,

    pub border_left_width: f64,
    pub border_left_color: String,
    pub border_right_width: f64,
    pub border_right_color: String,
    pub border_radius: String,
    pub box_shadow: String,
    pub background_image: String,
    pub backdrop_filter: String,
    pub overflow: String,
    pub white_space: String,
    pub position: String,
    /// True when a background-image sits between this text and the first
    /// opaque background color, so the painted backdrop is not a readable
    /// color. Contrast abstains in that case rather than guessing.
    pub bg_uncertain: bool,
    /// Visually-hidden (screen-reader-only) text. Visual rules must skip it.
    pub sr_only: bool,
    pub opacity: f64,
    /// True when the element carries a click handler attribute. Computed style
    /// cannot express this, so the probe reports it explicitly.
    pub has_click: bool,
    pub cursor: String,

    pub width: f64,
    pub height: f64,
    pub x: f64,
    pub y: f64,
    /// Content width, used to detect overflow the user would actually see.
    pub scroll_width: f64,
    pub scroll_height: f64,
    pub client_width: f64,
    pub client_height: f64,
}

impl Element {
    /// Stable, human-meaningful location for a finding.
    pub fn location(&self) -> String {
        let mut s = self.tag.clone();
        if !self.id.is_empty() {
            s.push('#');
            s.push_str(&self.id);
        } else if !self.class.is_empty() {
            let first: Vec<&str> = self.class.split_whitespace().take(3).collect();
            if !first.is_empty() {
                s.push('.');
                s.push_str(&first.join("."));
            }
        }
        if !self.own_text.is_empty() {
            let t: String = self.own_text.chars().take(28).collect();
            s.push_str(&format!(" \"{}\"", t));
        }
        s
    }

    pub fn parent_tag(&self) -> Option<&str> {
        self.ancestors.last().map(|s| s.as_str())
    }
}

/// The in-page probe. Returns one JSON row per element. `effectiveBg` walks
/// ancestors for the first opaque background so the Rust side never has to
/// guess what a translucent layer sits on.
const PROBE_JS: &str = r#"
(() => {
  const rows = [];
  const opaque = (c) => {
    if (!c) return false;
    if (c === 'transparent') return false;
    const m = c.match(/rgba?\(([^)]+)\)/);
    if (!m) return true;
    const p = m[1].split(/[,\s\/]+/).filter(Boolean);
    return p.length < 4 || parseFloat(p[3]) >= 1;
  };
  const effBg = (el) => {
    let n = el;
    while (n) {
      const s = getComputedStyle(n);
      if (opaque(s.backgroundColor)) return s.backgroundColor;
      n = n.parentElement;
    }
    return 'rgb(255, 255, 255)';
  };
  // True when a background-image (photo, gradient, mesh) sits between the text
  // and the first opaque background color. The painted backdrop is then not a
  // color we can read, so contrast must abstain rather than compare against
  // whatever opaque color happens to be further up the tree.
  const bgUncertain = (el) => {
    let n = el;
    while (n) {
      const s = getComputedStyle(n);
      if (s.backgroundImage && s.backgroundImage !== 'none') return true;
      if (opaque(s.backgroundColor)) return false;
      n = n.parentElement;
    }
    return false;
  };
  // Screen-reader-only text is not visually present, so visual rules must skip
  // it. The clip/clip-path + 1px box idiom is the common implementation.
  const srOnly = (el, s, r) => {
    if (r.width <= 1 && r.height <= 1) return true;
    if (s.clipPath && s.clipPath !== 'none' && r.width <= 2) return true;
    return s.clip === 'rect(0px, 0px, 0px, 0px)';
  };
  const all = document.querySelectorAll('*');
  let i = -1;
  for (const el of all) {
    i++;
    const tag = el.tagName.toLowerCase();
    if (tag === 'script' || tag === 'style' || tag === 'meta' || tag === 'link' || tag === 'head') continue;
    const s = getComputedStyle(el);
    if (s.display === 'none' || s.visibility === 'hidden') continue;
    const r = el.getBoundingClientRect();
    const anc = [];
    let p = el.parentElement;
    while (p) { anc.unshift(p.tagName.toLowerCase()); p = p.parentElement; }
    let own = '';
    for (const n of el.childNodes) if (n.nodeType === 3) own += n.nodeValue;
    rows.push({
      tag, class: (typeof el.className === 'string' ? el.className : ''), id: el.id || '', index: i,
      ancestors: anc,
      text: (el.textContent || '').trim().slice(0, 120),
      ownText: own.trim().slice(0, 120),
      color: s.color, background: s.backgroundColor, effectiveBg: effBg(el),
      bgUncertain: bgUncertain(el), srOnly: srOnly(el, s, r),
      opacity: parseFloat(s.opacity),
      fontFamily: s.fontFamily, fontSize: parseFloat(s.fontSize) || 0,
      fontWeight: parseInt(s.fontWeight, 10) || 400,
      letterSpacing: s.letterSpacing, lineHeight: s.lineHeight,
      textTransform: s.textTransform, textAlign: s.textAlign,
      borderLeftWidth: parseFloat(s.borderLeftWidth) || 0, borderLeftColor: s.borderLeftColor,
      borderRightWidth: parseFloat(s.borderRightWidth) || 0, borderRightColor: s.borderRightColor,
      borderRadius: s.borderRadius, boxShadow: s.boxShadow,
      backgroundImage: s.backgroundImage, backdropFilter: s.backdropFilter || s.webkitBackdropFilter || 'none',
      overflow: s.overflow, whiteSpace: s.whiteSpace, position: s.position,
      hasClick: !!(el.getAttribute('onclick') || el.getAttribute('@click') || el.getAttribute('v-on:click')),
      cursor: s.cursor,
      width: r.width, height: r.height, x: r.x, y: r.y,
      scrollWidth: el.scrollWidth, scrollHeight: el.scrollHeight,
      clientWidth: el.clientWidth, clientHeight: el.clientHeight
    });
  }
  return JSON.stringify(rows);
})()
"#;

/// Executable names/paths tried in order. Honors `CHROME` first so a user can
/// point at any Chromium build.
fn candidates() -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(p) = std::env::var("CHROME") {
        if !p.trim().is_empty() {
            v.push(p);
        }
    }
    v.extend(
        [
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "/usr/bin/chromium",
            "/usr/bin/google-chrome-stable",
            "/opt/google/chrome/chrome",
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    v
}

/// Resolve a usable browser binary, or explain why none was found.
pub fn find_browser() -> Result<String, String> {
    for c in candidates() {
        if c.contains('/') {
            if Path::new(&c).is_file() {
                return Ok(c);
            }
            continue;
        }
        // Bare name: probe with --version so we respect PATH.
        if let Ok(out) = Command::new(&c)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            if out.success() {
                return Ok(c);
            }
        }
    }
    Err("no Chrome/Chromium found (set CHROME=/path/to/chrome)".to_string())
}

/// Kills the browser on drop so a lint run can never leak a process.
struct BrowserGuard(Child, std::path::PathBuf);

impl Drop for BrowserGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_dir_all(&self.1);
    }
}

/// Launch headless Chrome with a throwaway profile and return its guard plus
/// the DevTools WebSocket URL.
fn launch(browser: &str) -> Result<(BrowserGuard, String), String> {
    let profile = std::env::temp_dir().join(format!(
        "formwork-design-lint-{}-{}",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    std::fs::create_dir_all(&profile)
        .map_err(|e| format!("cannot create browser profile dir: {}", e))?;

    let child = Command::new(browser)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-sandbox",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-extensions",
            "--disable-background-networking",
            "--remote-debugging-port=0",
            "--window-size=1280,900",
            &format!("--user-data-dir={}", profile.display()),
            "about:blank",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot launch {}: {}", browser, e))?;

    let mut guard = BrowserGuard(child, profile);

    // Chrome prints "DevTools listening on ws://..." to stderr. With port 0 it
    // picks a free port, so reading the line is the only way to learn it.
    let stderr = guard
        .0
        .stderr
        .take()
        .ok_or_else(|| "browser stderr unavailable".to_string())?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if let Some(i) = line.find("ws://") {
                        let _ = tx.send(line[i..].trim().to_string());
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let ws = rx
        .recv_timeout(Duration::from_secs(20))
        .map_err(|_| "browser did not report a DevTools endpoint within 20s".to_string())?;
    Ok((guard, ws))
}

/// A minimal blocking CDP session over one WebSocket.
struct Session {
    ws: tungstenite::WebSocket<TcpStream>,
    next_id: u64,
}

impl Session {
    fn connect(ws_url: &str) -> Result<Self, String> {
        // Parse ws://host:port/path by hand to avoid pulling in a URL crate.
        let rest = ws_url
            .strip_prefix("ws://")
            .ok_or_else(|| format!("unsupported DevTools URL: {}", ws_url))?;
        let (authority, _) = rest.split_once('/').unwrap_or((rest, ""));
        let stream = TcpStream::connect(authority)
            .map_err(|e| format!("cannot connect to DevTools at {}: {}", authority, e))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .map_err(|e| format!("cannot set read timeout: {}", e))?;
        let (ws, _) = tungstenite::client::client(
            // A hand-built request must carry the full WebSocket upgrade header
            // set; tungstenite validates the key it echoes back, so reuse its
            // own key generator rather than inventing one.
            tungstenite::handshake::client::Request::builder()
                .uri(ws_url)
                .header("Host", authority)
                .header("Connection", "Upgrade")
                .header("Upgrade", "websocket")
                .header("Sec-WebSocket-Version", "13")
                .header(
                    "Sec-WebSocket-Key",
                    tungstenite::handshake::client::generate_key(),
                )
                .body(())
                .map_err(|e| format!("bad DevTools request: {}", e))?,
            stream,
        )
        .map_err(|e| format!("DevTools handshake failed: {}", e))?;
        Ok(Session { ws, next_id: 0 })
    }

    /// Send one command and block until its matching reply arrives, skipping
    /// the event stream.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let msg = serde_json::json!({"id": id, "method": method, "params": params});
        self.ws
            .send(tungstenite::Message::Text(msg.to_string().into()))
            .map_err(|e| format!("CDP send failed ({}): {}", method, e))?;

        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if Instant::now() > deadline {
                return Err(format!("CDP timeout waiting for {}", method));
            }
            let msg = self
                .ws
                .read()
                .map_err(|e| format!("CDP read failed ({}): {}", method, e))?;
            let text = match msg {
                tungstenite::Message::Text(t) => t.to_string(),
                tungstenite::Message::Close(_) => {
                    return Err("DevTools closed the connection".to_string())
                }
                _ => continue,
            };
            let v: Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if v.get("id").and_then(|i| i.as_u64()) == Some(id) {
                if let Some(err) = v.get("error") {
                    return Err(format!("CDP error on {}: {}", method, err));
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }
}

/// Render `path` in a headless browser and return its elements with computed
/// styles and geometry.
pub fn probe(path: &Path) -> Result<Vec<Element>, String> {
    let browser = find_browser()?;
    let (guard, ws_url) = launch(&browser)?;
    let mut s = Session::connect(&ws_url)?;

    // Attach to a page target. The browser-level endpoint can't evaluate JS,
    // so create a tab and talk to it via a flattened session.
    let abs = std::fs::canonicalize(path)
        .map_err(|e| format!("cannot resolve {}: {}", path.display(), e))?;
    let url = format!("file://{}", abs.display());

    let target = s.call(
        "Target.createTarget",
        serde_json::json!({"url": "about:blank"}),
    )?;
    let target_id = target
        .get("targetId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "CDP did not return a targetId".to_string())?
        .to_string();
    let attached = s.call(
        "Target.attachToTarget",
        serde_json::json!({"targetId": target_id, "flatten": true}),
    )?;
    let session_id = attached
        .get("sessionId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "CDP did not return a sessionId".to_string())?
        .to_string();

    // With `flatten`, sessionId rides along on each command.
    let mut call_in_page = |method: &str, params: Value| -> Result<Value, String> {
        s.next_id += 1;
        let id = s.next_id;
        let msg = serde_json::json!({
            "id": id, "method": method, "params": params, "sessionId": session_id
        });
        s.ws.send(tungstenite::Message::Text(msg.to_string().into()))
            .map_err(|e| format!("CDP send failed ({}): {}", method, e))?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if Instant::now() > deadline {
                return Err(format!("CDP timeout waiting for {}", method));
            }
            let m =
                s.ws.read()
                    .map_err(|e| format!("CDP read failed ({}): {}", method, e))?;
            let text = match m {
                tungstenite::Message::Text(t) => t.to_string(),
                tungstenite::Message::Close(_) => {
                    return Err("DevTools closed the connection".to_string())
                }
                _ => continue,
            };
            let v: Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if v.get("id").and_then(|i| i.as_u64()) == Some(id) {
                if let Some(err) = v.get("error") {
                    return Err(format!("CDP error on {}: {}", method, err));
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    };

    call_in_page("Page.enable", serde_json::json!({}))?;
    call_in_page("Runtime.enable", serde_json::json!({}))?;
    call_in_page("Page.navigate", serde_json::json!({"url": url}))?;

    // Wait for the document to finish and for any CDN stylesheet (Tailwind) to
    // apply. Poll readyState rather than sleeping a fixed interval.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let r = call_in_page(
            "Runtime.evaluate",
            serde_json::json!({"expression": "document.readyState", "returnByValue": true}),
        )?;
        let state = r
            .pointer("/result/value")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if state == "complete" {
            break;
        }
        if Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(120));
    }
    // One settle pass: a CDN-injected stylesheet applies a tick after load.
    std::thread::sleep(Duration::from_millis(250));

    let out = call_in_page(
        "Runtime.evaluate",
        serde_json::json!({"expression": PROBE_JS, "returnByValue": true, "awaitPromise": false}),
    )?;
    if let Some(exc) = out.get("exceptionDetails") {
        return Err(format!("probe script failed: {}", exc));
    }
    let raw = out
        .pointer("/result/value")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "probe script returned no value".to_string())?;
    let elements = elements_from_json(raw)?;

    // Explicit: kills the browser and removes the throwaway profile. Dropping
    // here (rather than at scope end) keeps the process lifetime obvious.
    drop(guard);
    Ok(elements)
}

/// Parse the probe's JSON payload into elements. Split out from `probe` so the
/// deserialization — the one part that runs on data we don't control — is
/// testable without a browser. Every field is read defensively (`s_of`/`f_of`
/// tolerate wrong types and missing keys), so a malformed row degrades to
/// empty/zero fields rather than crashing the lint run.
pub fn elements_from_json(raw: &str) -> Result<Vec<Element>, String> {
    let rows: Vec<Value> =
        serde_json::from_str(raw).map_err(|e| format!("probe output was not JSON: {}", e))?;
    Ok(rows.iter().map(row_to_element).collect())
}

fn s_of(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn f_of(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0)
}

fn row_to_element(v: &Value) -> Element {
    Element {
        tag: s_of(v, "tag"),
        class: s_of(v, "class"),
        id: s_of(v, "id"),
        index: f_of(v, "index") as usize,
        ancestors: v
            .get("ancestors")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        text: s_of(v, "text"),
        own_text: s_of(v, "ownText"),
        color: s_of(v, "color"),
        background: s_of(v, "background"),
        effective_bg: s_of(v, "effectiveBg"),
        font_family: s_of(v, "fontFamily"),
        font_size_px: f_of(v, "fontSize"),
        font_weight: f_of(v, "fontWeight") as u16,
        letter_spacing: s_of(v, "letterSpacing"),
        line_height: s_of(v, "lineHeight"),
        text_transform: s_of(v, "textTransform"),
        text_align: s_of(v, "textAlign"),
        border_left_width: f_of(v, "borderLeftWidth"),
        border_left_color: s_of(v, "borderLeftColor"),
        border_right_width: f_of(v, "borderRightWidth"),
        border_right_color: s_of(v, "borderRightColor"),
        border_radius: s_of(v, "borderRadius"),
        box_shadow: s_of(v, "boxShadow"),
        background_image: s_of(v, "backgroundImage"),
        backdrop_filter: s_of(v, "backdropFilter"),
        overflow: s_of(v, "overflow"),
        white_space: s_of(v, "whiteSpace"),
        position: s_of(v, "position"),
        bg_uncertain: v
            .get("bgUncertain")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        sr_only: v.get("srOnly").and_then(|x| x.as_bool()).unwrap_or(false),
        opacity: v.get("opacity").and_then(|x| x.as_f64()).unwrap_or(1.0),
        has_click: v.get("hasClick").and_then(|x| x.as_bool()).unwrap_or(false),
        cursor: s_of(v, "cursor"),
        width: f_of(v, "width"),
        height: f_of(v, "height"),
        x: f_of(v, "x"),
        y: f_of(v, "y"),
        scroll_width: f_of(v, "scrollWidth"),
        scroll_height: f_of(v, "scrollHeight"),
        client_width: f_of(v, "clientWidth"),
        client_height: f_of(v, "clientHeight"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design_lint::rules;

    /// The probe payload comes from a browser we then feed to the rule engine.
    /// Deserialization must never panic and must tolerate every malformation:
    /// wrong types, missing keys, nulls, non-arrays. A crash here would fail an
    /// otherwise valid design submission.
    #[test]
    fn elements_from_json_tolerates_malformed_payloads() {
        let ok = |raw: &str| {
            let r = elements_from_json(raw);
            // Either valid JSON (parsed, possibly to defaulted rows) or a clean
            // Err — never a panic.
            let _ = r;
        };
        ok("[]");
        ok("[{}]"); // row with no fields -> all defaults
        ok(r#"[{"tag":123,"fontSize":"not a number","ancestors":"nope"}]"#);
        ok(r#"[{"tag":null,"color":null,"width":null}]"#);
        ok(r#"[{"fontSize":"NaN"},{"width":"-1e999"}]"#);
        ok(r#"[{"ancestors":[1,2,{"x":3}],"tag":"div"}]"#);
        ok(r#"[{"borderRadius":{"nested":"object"}}]"#);
        // Not even the right shape:
        ok("{}");
        ok("null");
        ok("42");
        ok(r#""a string""#);
        // Genuinely broken JSON -> Err, not panic.
        assert!(elements_from_json("{not json").is_err());
        assert!(elements_from_json("").is_err());
    }

    /// A minimal-but-valid row must still yield a usable element with sane
    /// defaults, and those defaults must be safe to feed to the rule engine.
    #[test]
    fn defaulted_row_is_safe_for_rules() {
        let els = elements_from_json(r#"[{"tag":"div"}]"#).unwrap();
        assert_eq!(els.len(), 1);
        let e = &els[0];
        assert_eq!(e.tag, "div");
        assert_eq!(e.font_size_px, 0.0);
        assert!(e.color.is_empty());
        // The rule engine must not panic on defaulted rows.
        let _ = rules::run(&els, None);
    }
}
