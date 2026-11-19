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
                marks[d].push(format!(
                    r##"<a href="#f{n}"><circle id="{id}" class="mark {c}" cx="{x}" cy="{y}" r="{r}"><title>{}</title></circle></a>"##,
                    escape(&format!("{} {}: {}", f.rule, f.reference, f.message))
                ));
                links.push(format!(
                    r##"<a class="goto" href="#{id}" data-view="{d}" data-x="{x}" data-y="{y}">{}</a>"##,
                    escape(&drawing.title)
                ));
            }
        }
        let case = f
            .case
            .as_deref()
            .map(|c| format!(r#"<span class="case">fault "{}"</span>"#, escape(c)))
            .unwrap_or_default();
        let _ = write!(
            list,
            r#"<li id="f{n}" class="finding {c}"><div class="head"><span class="badge {c}">{}</span><code>{}</code> <b>{}</b>{case}</div><p>{}</p><div class="links">{}</div></li>"#,
            c,
            escape(f.rule),
            escape(&f.reference),
            escape(&f.message),
            links.join(" ")
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
<style>{STYLE}</style></head>
<body>
<header><h1>{title}</h1>
<div class="counts"><label><input type="checkbox" data-show="error" checked> {e} errors</label>
<label><input type="checkbox" data-show="warning" checked> {w} warnings</label>
<label><input type="checkbox" data-show="unknown"> {u} unknown</label>
<label><input type="checkbox" data-show="pass"> {p} passed</label></div></header>
<main>
<section class="drawings">{drawn}</section>
<section class="list"><ol>{list}</ol></section>
</main>
<script>{SCRIPT}</script>
</body></html>
"#,
        title = escape(title),
        drawn = match note {
            Some(n) => format!(r#"<p class="hint">No drawings: {}</p>"#, escape(n)),
            None => format!(
                r#"<nav>{tabs}</nav>{views}<p class="hint">Scroll to zoom, drag to pan, double-click to see it all. Click a ring to see its finding.</p>"#
            ),
        },
        e = report.errors(),
        w = report.warnings(),
        u = report.unknowns(),
        p = report.passes(),
    );
    page
}

const STYLE: &str = r#"
:root{--bg:#fbfbfa;--fg:#1d1d1b;--muted:#6b6b66;--line:#deddd8;--card:#ffffff;--error:#c62828;--warning:#b26a00;--unknown:#5c6bc0;--pass:#2e7d32}
@media (prefers-color-scheme: dark){:root:not([data-theme="light"]){--bg:#181816;--fg:#ecebe6;--muted:#a3a29b;--line:#34332f;--card:#22211e;--error:#ef6b6b;--warning:#f0a43a;--unknown:#9aa5e8;--pass:#6cc070}}
:root[data-theme="dark"]{--bg:#181816;--fg:#ecebe6;--muted:#a3a29b;--line:#34332f;--card:#22211e;--error:#ef6b6b;--warning:#f0a43a;--unknown:#9aa5e8;--pass:#6cc070}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:14px/1.45 system-ui,sans-serif}
header{padding:12px 16px;border-bottom:1px solid var(--line);display:flex;flex-wrap:wrap;gap:8px 24px;align-items:baseline}
h1{font-size:18px;margin:0}.counts{display:flex;gap:16px;flex-wrap:wrap;color:var(--muted)}
main{display:grid;grid-template-columns:minmax(0,3fr) minmax(280px,2fr);height:calc(100vh - 54px)}
@media (max-width:800px){main{grid-template-columns:1fr;height:auto}.view{height:60vh}}
.drawings{display:flex;flex-direction:column;min-width:0;border-right:1px solid var(--line)}
nav{display:flex;gap:4px;padding:8px 16px;flex-wrap:wrap}.tab{border:1px solid var(--line);background:var(--card);color:var(--fg);padding:4px 10px;border-radius:6px;cursor:pointer}
.tab.on{border-color:var(--fg)}.view{display:none;flex:1;overflow:hidden;background:#f5f4ef;cursor:grab}.view.on{display:block}
.view svg{width:100%;height:100%}.hint{margin:4px 16px 8px;color:var(--muted);font-size:12px}
.mark{fill:none;stroke-width:1.2;pointer-events:visibleFill;cursor:pointer}.mark.error{stroke:#c62828}.mark.warning{stroke:#e08a00}.mark.unknown{stroke:#5c6bc0}.mark.pass{stroke:#2e7d32}
.mark.hidden{display:none}.mark.hot{stroke-width:2;animation:pulse 1s ease-in-out 3}@keyframes pulse{50%{stroke-opacity:.3}}
.list{overflow:auto;padding:0 16px}.list ol{list-style:none;margin:0;padding:8px 0}
.finding{background:var(--card);border:1px solid var(--line);border-left:4px solid var(--muted);border-radius:6px;padding:8px 12px;margin:8px 0}
.finding.error{border-left-color:var(--error)}.finding.warning{border-left-color:var(--warning)}.finding.unknown{border-left-color:var(--unknown)}.finding.pass{border-left-color:var(--pass)}
.finding.hidden{display:none}.finding:target{outline:2px solid var(--fg)}
.finding p{margin:4px 0;word-break:break-word}.head{display:flex;gap:8px;align-items:baseline;flex-wrap:wrap}
.badge{font-size:11px;text-transform:uppercase;font-weight:600}.badge.error{color:var(--error)}.badge.warning{color:var(--warning)}.badge.unknown{color:var(--unknown)}.badge.pass{color:var(--pass)}
.case{color:var(--muted)}.links{display:flex;gap:8px;flex-wrap:wrap}.goto{color:var(--fg)}
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
 document.querySelectorAll('.mark').forEach(m=>m.classList.toggle('hidden',![...on].some(c=>m.classList.contains(c))))}
document.querySelectorAll('[data-show]').forEach(c=>c.onchange=filter);filter();
";
