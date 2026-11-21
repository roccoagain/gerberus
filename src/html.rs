//! `check --html`: the findings as one self-contained web page, each marked on the schematic and the board.
//!
//! KiCad's own `kicad-cli` draws every schematic sheet and the board as SVG. Each finding about a part gets a ring
//! on every sheet where one of its symbols sits and on the board where its footprint sits, and the list of findings
//! links to those rings and back. The page holds everything (drawings, styles, script), so it can be mailed or
//! attached as one file.
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::design::Design;
use crate::diag::plural;
use crate::finding::{Finding, Severity, Verdict};
use crate::kicad::pcb::{Layout, PlacedFootprint, Point};
use crate::rules::Report;

/// One drawing: a schematic sheet (by its sheet path, `/` for the root) or the board.
pub struct Drawing {
    pub title: String,
    /// The sheet path for a schematic sheet; `None` for the board.
    pub sheet: Option<String>,
    pub svg: String,
}

/// A scratch folder that is removed when dropped.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn kicad_cli(args: &[&str]) -> Result<(), String> {
    let out = Command::new("kicad-cli").args(args).output().map_err(|e| {
        format!("can't run kicad-cli ({e}); --html needs KiCad's kicad-cli on the PATH to draw the schematic and board")
    })?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "kicad-cli {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Draws every sheet of the schematic and, when there is one, the board, with `kicad-cli`.
pub fn draw(design: &Design, schematic: &Path, pcb: Option<&Path>) -> Result<Vec<Drawing>, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("gerberus-html-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let scratch = Scratch(dir);
    let out = scratch.0.to_string_lossy().to_string();
    kicad_cli(&["sch", "export", "svg", "-o", &out, &schematic.to_string_lossy()])?;
    // kicad-cli names each sheet's file after the root and the sheet path, `-` between names and `_` for a
    // backslash in one: `board.svg`, `board-Power.svg`.
    let stem = schematic.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let mut sheets: Vec<String> = design
        .components
        .values()
        .flat_map(|c| c.placed.iter().map(|p| p.sheet.clone()))
        .collect();
    sheets.push("/".into());
    sheets.sort();
    sheets.dedup();
    let mut drawings = vec![];
    for sheet in sheets {
        let name = sheet.trim_matches('/').replace('/', "-").replace('\\', "_");
        let file = if name.is_empty() {
            format!("{stem}.svg")
        } else {
            format!("{stem}-{name}.svg")
        };
        let path = scratch.0.join(&file);
        let svg = match std::fs::read_to_string(&path) {
            Ok(svg) => svg,
            Err(e) => {
                eprintln!("warning: sheet {sheet} isn't drawn: {}: {e}", path.display());
                continue;
            }
        };
        drawings.push(Drawing {
            title: if name.is_empty() {
                "Schematic".into()
            } else {
                format!("Sheet {}", sheet.trim_matches('/'))
            },
            sheet: Some(sheet),
            svg,
        });
    }
    if let Some(pcb) = pcb {
        let file = scratch.0.join("board.svg");
        kicad_cli(&[
            "pcb",
            "export",
            "svg",
            "--mode-single",
            "--layers",
            "F.Cu,B.Cu,F.SilkS,B.SilkS,Edge.Cuts",
            "--page-size-mode",
            "1",
            "--exclude-drawing-sheet",
            "-o",
            &file.to_string_lossy(),
            &pcb.to_string_lossy(),
        ])?;
        let svg = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        drawings.push(Drawing {
            title: "Board".into(),
            sheet: None,
            svg,
        });
    }
    Ok(drawings)
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The finding's class for its colour: error, warning, unknown or pass.
fn class(f: &Finding) -> &'static str {
    match (f.verdict, f.severity) {
        (Verdict::Fail, Severity::Error) => "error",
        (Verdict::Fail, Severity::Warning) => "warning",
        (Verdict::Unknown, _) => "unknown",
        (Verdict::Pass, _) => "pass",
    }
}

/// The SVG's `<svg …>` element's content, without the XML prologue, so it can sit inside the page.
fn inner(svg: &str) -> &str {
    svg.find("<svg").map_or(svg, |i| &svg[i..])
}

/// The footprints' extent with a 5 mm margin, as an SVG view box: x, y, width, height.
fn board_box(layout: &Layout) -> Option<[f64; 4]> {
    let points = layout.footprints.iter().flat_map(|f| f.pads.iter().map(|p| p.at));
    let (mut lo, mut hi) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
    for (x, y) in points {
        lo = (lo.0.min(x), lo.1.min(y));
        hi = (hi.0.max(x), hi.1.max(y));
    }
    (lo.0 <= hi.0).then_some([lo.0 - 5.0, lo.1 - 5.0, hi.0 - lo.0 + 10.0, hi.1 - lo.1 + 10.0])
}

/// `svg` with its opening tag's `viewBox` set to `b`.
fn with_view_box(svg: &str, b: [f64; 4]) -> String {
    let tag = svg.find('>').unwrap_or(svg.len());
    let view = format!(r#"viewBox="{} {} {} {}""#, b[0], b[1], b[2], b[3]);
    match svg[..tag].find("viewBox=\"") {
        Some(i) => {
            let end = i + 9 + svg[i + 9..].find('"').unwrap_or(0) + 1;
            format!("{}{view}{}", &svg[..i], &svg[end..])
        }
        None => format!("<svg {view}{}", &svg[4..]),
    }
}

/// The middle of a footprint's pads, where its ring goes: a footprint's origin is often its pin 1.
fn middle(fp: &PlacedFootprint) -> Point {
    let (mut lo, mut hi) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
    for p in &fp.pads {
        lo = (lo.0.min(p.at.0), lo.1.min(p.at.1));
        hi = (hi.0.max(p.at.0), hi.1.max(p.at.1));
    }
    if fp.pads.is_empty() {
        fp.at
    } else {
        (f64::midpoint(lo.0, hi.0), f64::midpoint(lo.1, hi.1))
    }
}

/// KiCad's `<title>` names the program that drew the SVG; left in, it shows as a tooltip over the whole drawing.
fn without_title(svg: &str) -> String {
    match (svg.find("<title>"), svg.find("</title>")) {
        (Some(a), Some(b)) if a < b => format!("{}{}", &svg[..a], &svg[b + "</title>".len()..]),
        _ => svg.to_string(),
    }
}

/// The page. `layout` places the footprints for the board drawing's marks; `note` says why there are no drawings.
pub fn render(
    design: &Design,
    report: &Report,
    drawings: &[Drawing],
    layout: Option<&Layout>,
    title: &str,
    note: Option<&str>,
) -> String {
    let findings = &report.findings;
    // Errors, then warnings, then UNKNOWNs, then passes.
    let mut order: Vec<usize> = (0..findings.len()).collect();
    order.sort_by_key(|&i| {
        let f = &findings[i];
        (
            ["error", "warning", "unknown", "pass"]
                .iter()
                .position(|c| *c == class(f))
                .unwrap_or(4),
            f.reference.clone(),
        )
    });
    // Each drawing's rings, in list order; drawn in reverse so an error's ring lies over a pass's on the same part.
    let mut marks: Vec<Vec<String>> = vec![vec![]; drawings.len()];
    let mut list = String::new();
    for (n, &i) in order.iter().enumerate() {
        let f = &findings[i];
        let c = class(f);
        let shown = crate::rules::laid_out(f);
        let mut links = vec![];
        for (d, drawing) in drawings.iter().enumerate() {
            let points: Vec<(f64, f64)> = match &drawing.sheet {
                Some(sheet) => design
                    .components
                    .get(&f.reference)
                    .map(|comp| comp.placed.iter().filter(|p| &p.sheet == sheet).map(|p| p.at).collect())
                    .unwrap_or_default(),
                None => layout
                    .and_then(|l| l.footprints.iter().find(|fp| fp.reference == f.reference))
                    .map(|fp| vec![middle(fp)])
                    .unwrap_or_default(),
            };
            // Schematic symbols are larger than footprints.
            let r = if drawing.sheet.is_some() { 6 } else { 3 };
            for (k, (x, y)) in points.into_iter().enumerate() {
                let id = format!("m{n}-{d}-{k}");
                // A halo under the coloured ring keeps it clear of copper drawn in the same colour.
                marks[d].push(format!(
                    r##"<a class="ring {c}" href="#f{n}"><circle class="halo" cx="{x}" cy="{y}" r="{r}"/><circle id="{id}" class="mark {c}" cx="{x}" cy="{y}" r="{r}"><title>{}</title></circle></a>"##,
                    escape(&format!("{} {}: {}", f.rule, f.reference, shown.headline))
                ));
                links.push(format!(
                    r##"<a class="goto" href="#{id}" data-view="{d}" data-x="{x}" data-y="{y}">{}</a>"##,
                    escape(&drawing.title)
                ));
            }
        }
        // The headline and help, as the terminal prints them; the notes fold away under them.
        let help = shown
            .help
            .as_ref()
            .map(|h| format!(r#"<p class="help">{}</p>"#, escape(h)));
        let notes = (!shown.notes.is_empty()).then(|| {
            let items = shown.notes.iter().fold(String::new(), |mut out, n| {
                let _ = write!(out, "<li>{}</li>", escape(n));
                out
            });
            format!("<details><summary>details</summary><ul>{items}</ul></details>")
        });
        let _ = write!(
            list,
            r#"<li id="f{n}" class="finding {c}"><div class="head"><i class="dot" role="img" aria-label="{c}" title="{c}"></i><b>{}</b><code>{}</code></div><p>{}</p>{}{}<div class="links">{}</div></li>"#,
            escape(shown.location.as_deref().unwrap_or(&f.reference)),
            escape(f.rule),
            escape(&shown.headline),
            help.unwrap_or_default(),
            notes.unwrap_or_default(),
            links.join("")
        );
    }
    let mut views = String::new();
    let mut tabs = String::new();
    for (d, drawing) in drawings.iter().enumerate() {
        let svg = inner(&drawing.svg);
        let close = svg.rfind("</svg>").unwrap_or(svg.len());
        // The board is drawn on its whole page; open it on the board itself.
        let svg = match (&drawing.sheet, layout.and_then(board_box)) {
            (None, Some(b)) => with_view_box(&without_title(&svg[..close]), b),
            _ => without_title(&svg[..close]),
        };
        // KiCad's drawing in its own group, so dark mode can invert a schematic sheet without touching the rings.
        let open = svg.find('>').map_or(svg.len(), |i| i + 1);
        let art = if drawing.sheet.is_some() { "art sheet" } else { "art" };
        let svg = format!(r#"{}<g class="{art}">{}</g>"#, &svg[..open], &svg[open..]);
        let _ = write!(
            tabs,
            r#"<button class="tab" data-view="{d}">{}</button>"#,
            escape(&drawing.title)
        );
        let _ = write!(
            views,
            r#"<div class="view" id="v{d}">{}<g class="marks">{}</g></svg></div>"#,
            svg,
            marks[d].iter().rev().cloned().collect::<String>()
        );
    }
    let mut page = String::new();
    let _ = write!(
        page,
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<link rel="icon" href="{icon}">
<style>{STYLE}</style>
<script>try{{const t=localStorage.getItem('gerberus-theme');if(t)document.documentElement.dataset.theme=t}}catch(e){{}}</script></head>
<body>
<header>{LOGO}<h1>{title}</h1>
<div class="filters"><label class="error"><input type="checkbox" data-show="error" checked><i class="dot"></i>{e}</label>
<label class="warning"><input type="checkbox" data-show="warning" checked><i class="dot"></i>{w}</label>
<label class="unknown"><input type="checkbox" data-show="unknown"><i class="dot"></i>{u} unknown</label>
<label class="pass"><input type="checkbox" data-show="pass"><i class="dot"></i>{p} passed</label></div>
<button class="theme" aria-label="Switch light or dark"><svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M8 2a6 6 0 0 1 0 12z" fill="currentColor"/></svg></button></header>
<main>
<section class="drawings">{drawn}</section>
<section class="list"><ol>{list}</ol></section>
</main>
<script>{SCRIPT}</script>
</body></html>
"#,
        title = escape(title),
        icon = data_url(LOGO),
        drawn = match note {
            Some(n) => format!(r#"<p class="note">No drawings: {}</p>"#, escape(n)),
            None => format!(r"<nav>{tabs}</nav>{views}"),
        },
        e = plural(report.errors(), "error"),
        w = plural(report.warnings(), "warning"),
        u = report.unknowns(),
        p = report.passes(),
    );
    page
}

/// The Gerberus logo, a green three-headed dog, in the header and as the tab's icon.
const LOGO: &str = include_str!("../docs/logo.svg");

/// `svg` as a `data:` URL, for the page's icon.
fn data_url(svg: &str) -> String {
    let svg = svg
        .trim()
        .replace('"', "'")
        .replace('%', "%25")
        .replace('#', "%23")
        .replace('<', "%3C")
        .replace('>', "%3E")
        .replace('\n', " ");
    format!("data:image/svg+xml,{svg}")
}

const STYLE: &str = r#"
:root{--bg:#fff;--fg:#18181b;--muted:#71717a;--line:#ececef;--soft:#f6f6f7;--paper:#f5f4ef;--halo:#fff;--error:#dc2626;--warning:#d97706;--unknown:#6366f1;--pass:#16a34a}
@media (prefers-color-scheme: dark){:root:not([data-theme="light"]){--bg:#111113;--fg:#ededef;--muted:#8b8b93;--line:#26262a;--soft:#1c1c1f;--paper:#0b0b0d;--halo:#000;--error:#f87171;--warning:#fbbf24;--unknown:#a5b4fc;--pass:#4ade80}:root:not([data-theme="light"]) .art.sheet{filter:invert(1) hue-rotate(180deg)}}
:root[data-theme="dark"]{--bg:#111113;--fg:#ededef;--muted:#8b8b93;--line:#26262a;--soft:#1c1c1f;--paper:#0b0b0d;--halo:#000;--error:#f87171;--warning:#fbbf24;--unknown:#a5b4fc;--pass:#4ade80}
:root[data-theme="dark"] .art.sheet{filter:invert(1) hue-rotate(180deg)}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);font:13px/1.5 ui-sans-serif,system-ui,-apple-system,"Segoe UI",sans-serif;-webkit-font-smoothing:antialiased}
code{font:12px ui-monospace,SFMono-Regular,Menlo,monospace}
header{height:60px;padding:0 20px;border-bottom:1px solid var(--line);display:flex;gap:24px;align-items:center}
header>svg{width:38px;height:38px;flex:none;margin-right:-12px}
h1{font-size:14px;font-weight:600;margin:0;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.filters{display:flex;gap:20px;margin-left:auto;flex-wrap:wrap}
.filters label{display:flex;align-items:center;gap:6px;color:var(--muted);cursor:pointer;user-select:none;white-space:nowrap}
.filters label:hover,.filters label:has(:checked){color:var(--fg)}
.filters input{position:absolute;opacity:0;pointer-events:none}.filters label:not(:has(:checked)) .dot{opacity:.35}
.filters label:has(:focus-visible){outline:2px solid var(--fg);outline-offset:3px}
.theme{border:0;background:none;color:var(--muted);padding:4px;cursor:pointer;display:flex}.theme:hover{color:var(--fg)}.theme svg{width:16px;height:16px}
.dot{display:inline-block;width:7px;height:7px;border-radius:50%;flex:none;background:var(--muted)}
.error .dot{background:var(--error)}.warning .dot{background:var(--warning)}.unknown .dot{background:var(--unknown)}.pass .dot{background:var(--pass)}
main{display:grid;grid-template-columns:minmax(0,1fr) minmax(320px,420px);height:calc(100vh - 60px)}
@media (max-width:800px){header{height:auto;padding:12px 16px;flex-wrap:wrap}.filters{margin-left:0}main{grid-template-columns:1fr;height:auto}.view{height:60vh}}
.drawings{display:flex;flex-direction:column;min-width:0;border-right:1px solid var(--line)}
nav{display:flex;gap:20px;padding:0 20px;border-bottom:1px solid var(--line)}
.tab{border:0;background:none;color:var(--muted);font:inherit;padding:12px 0 10px;border-bottom:2px solid transparent;cursor:pointer}
.tab:hover{color:var(--fg)}.tab.on{color:var(--fg);border-bottom-color:var(--fg)}
.view{display:none;flex:1;overflow:hidden;background:var(--paper);cursor:grab}.view.on{display:block}.view:active{cursor:grabbing}
.view svg{width:100%;height:100%}.note{margin:20px;color:var(--muted)}
.halo,.mark{fill:none;vector-effect:non-scaling-stroke;pointer-events:visibleFill;cursor:pointer}
.halo{stroke:var(--halo);stroke-width:6px}.mark{stroke-width:2.5px}
.mark.error{stroke:#dc2626}.mark.warning{stroke:#f59e0b}.mark.unknown{stroke:#6366f1}.mark.pass{stroke:#16a34a}
.ring.hidden{display:none}.mark.hot{stroke-width:3.5px;animation:pulse 1s ease-in-out 3}@keyframes pulse{50%{stroke-opacity:.25}}
.list{overflow:auto}.list ol{list-style:none;margin:0;padding:0}
.finding{padding:14px 20px;border-bottom:1px solid var(--line);scroll-margin-top:8px}
.finding.hidden{display:none}.finding:target{background:var(--soft)}
.head{display:flex;gap:8px;align-items:center;flex-wrap:wrap}.head b{font-weight:600}.head code{color:var(--muted)}
.finding p{margin:4px 0 6px;color:var(--fg);opacity:.85;word-break:break-word}
.finding p.help{color:var(--muted);opacity:1}
details{margin:0 0 6px;color:var(--muted);font-size:12px}summary{cursor:pointer}details ul{margin:4px 0 0;padding-left:16px}
.links{display:flex;gap:12px;flex-wrap:wrap}.goto{color:var(--muted);text-decoration:none;font-size:12px}.goto:hover{color:var(--fg)}
.goto::after{content:" →"}
"#;

const SCRIPT: &str = r"
const views=[...document.querySelectorAll('.view')],tabs=[...document.querySelectorAll('.tab')];
const svgs=views.map(v=>v.querySelector('svg'));
const homes=svgs.map(s=>{const b=s.viewBox.baseVal;return[b.x,b.y,b.width,b.height]});
function show(d){views.forEach((v,i)=>v.classList.toggle('on',i==d));tabs.forEach((t,i)=>t.classList.toggle('on',i==d))}
function set(d,x,y,w,h){svgs[d].setAttribute('viewBox',[x,y,w,h].join(' '))}
// A point on the screen in the drawing's own units, allowing for the letterboxing that keeps its aspect ratio.
function at(s,cx,cy){const p=s.createSVGPoint();p.x=cx;p.y=cy;return p.matrixTransform(s.getScreenCTM().inverse())}
tabs.forEach(t=>t.onclick=()=>show(+t.dataset.view));
if(views.length)show(0);
views.forEach((v,d)=>{const s=svgs[d];let drag=null;
 v.addEventListener('wheel',e=>{e.preventDefault();const b=s.viewBox.baseVal,p=at(s,e.clientX,e.clientY),k=e.deltaY>0?1.2:1/1.2;
  set(d,p.x-(p.x-b.x)*k,p.y-(p.y-b.y)*k,b.width*k,b.height*k)},{passive:false});
 v.addEventListener('pointerdown',e=>{drag=at(s,e.clientX,e.clientY)});
 v.addEventListener('pointermove',e=>{if(!drag)return;const b=s.viewBox.baseVal,p=at(s,e.clientX,e.clientY);
  set(d,b.x-(p.x-drag.x),b.y-(p.y-drag.y),b.width,b.height)});
 v.addEventListener('dblclick',()=>set(d,...homes[d]));
 addEventListener('pointerup',()=>drag=null)});
document.querySelectorAll('.goto').forEach(a=>a.onclick=e=>{e.preventDefault();const d=+a.dataset.view,x=+a.dataset.x,y=+a.dataset.y;
 const w=homes[d][2]/4,h=homes[d][3]/4;show(d);set(d,x-w/2,y-h/2,w,h);
 document.querySelectorAll('.mark.hot').forEach(m=>m.classList.remove('hot'));
 const m=document.getElementById(a.getAttribute('href').slice(1));if(m)m.classList.add('hot')});
function filter(){const on=new Set([...document.querySelectorAll('[data-show]')].filter(c=>c.checked).map(c=>c.dataset.show));
 document.querySelectorAll('.finding').forEach(f=>f.classList.toggle('hidden',![...on].some(c=>f.classList.contains(c))));
 document.querySelectorAll('.ring').forEach(m=>m.classList.toggle('hidden',![...on].some(c=>m.classList.contains(c))))}
document.querySelectorAll('[data-show]').forEach(c=>c.onchange=filter);filter();
// The theme button flips between light and dark from whatever shows now, and the choice is kept for next time.
document.querySelector('.theme').onclick=()=>{const r=document.documentElement,t=r.dataset.theme||(matchMedia('(prefers-color-scheme: dark)').matches?'dark':'light');
 r.dataset.theme=t=='dark'?'light':'dark';try{localStorage.setItem('gerberus-theme',r.dataset.theme)}catch(e){}};
";
