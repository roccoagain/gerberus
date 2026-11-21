//! Plane splits: does every signal track keep one unbroken plane beneath it?
//!
//! A signal's return current flows in the plane next to its track. Where the copper under the track's two ends
//! is two separate pieces (a split between ground islands, or between a ground and a power pour), the return
//! current has to detour around the gap, which makes a loop that radiates and picks up noise.
//!
//! Every quarter millimetre along a track is matched to the filled pieces of copper beneath it on each neighbouring
//! copper layer. Pieces that overlap, such as two fills of one net, are one piece. A point inside a zone's outline
//! with no fill under it is over a hole (a via's or pad's clearance) and doesn't break the plane; a point outside
//! every outline has no plane. A track is referenced when, on some neighbouring layer, it has a plane all along it
//! and the pieces under it are one piece.
//!
//! Only nets that carry edges are checked: ones an output pin drives (output, bidirectional, tri-state or open
//! collector), with no power pin, and SPI and I²S lines by name (see `edges`). A supply rail or a strap tied by
//! resistors has no edges for a split to matter. One finding per such net with tracks and no zone of its own:
//!
//! - PASS: every track is referenced.
//! - FAIL (warning): some track isn't referenced, and runs over two or more separate pieces on a neighbouring layer.
//! - UNKNOWN: some track isn't referenced and has no plane under part of it, so where its return current flows
//!   isn't known.
use crate::design::{Design, PinKind};
use crate::edges::is_bus_line;
use crate::finding::{Finding, Severity, Verdict};
use crate::kicad::pcb::{Layout, Point, Track, inside};

/// A polygon with its bounding box, so most points are ruled out without walking its edges.
struct Shape<'a> {
    points: &'a [Point],
    lo: Point,
    hi: Point,
}

impl<'a> Shape<'a> {
    fn new(points: &'a [Point]) -> Shape<'a> {
        let lo = points
            .iter()
            .fold((f64::MAX, f64::MAX), |a, p| (a.0.min(p.0), a.1.min(p.1)));
        let hi = points
            .iter()
            .fold((f64::MIN, f64::MIN), |a, p| (a.0.max(p.0), a.1.max(p.1)));
        Shape { points, lo, hi }
    }

    fn contains(&self, p: Point) -> bool {
        p.0 >= self.lo.0 && p.0 <= self.hi.0 && p.1 >= self.lo.1 && p.1 <= self.hi.1 && inside(p, self.points)
    }
}

/// One copper layer's fills (each a piece) and zone outlines.
struct Plane<'a> {
    pieces: Vec<Shape<'a>>,
    outlines: Vec<Shape<'a>>,
}

impl<'a> Plane<'a> {
    fn new(layout: &'a Layout, layer: &str) -> Plane<'a> {
        let zones = || layout.zones.iter().filter(move |z| z.layer == layer);
        Plane {
            pieces: zones().flat_map(|z| z.polygons.iter().map(|p| Shape::new(p))).collect(),
            outlines: zones().flat_map(|z| z.outlines.iter().map(|p| Shape::new(p))).collect(),
        }
    }
}

/// How a track is referenced: one piece under it on some neighbouring layer, separate pieces, or no plane.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
enum Reference {
    Whole,
    Missing,
    Split,
}

/// The track on one plane.
fn on_plane(plane: &Plane<'_>, points: &[Point]) -> Reference {
    // Pieces joined by overlapping under a point, as a union-find forest.
    let mut parent: Vec<usize> = (0..plane.pieces.len()).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut under: Vec<usize> = vec![];
    let mut gap = false;
    for &p in points {
        let here: Vec<usize> = (0..plane.pieces.len())
            .filter(|&i| plane.pieces[i].contains(p))
            .collect();
        match here.first() {
            Some(&first) => {
                for &i in &here[1..] {
                    let (a, b) = (find(&mut parent, first), find(&mut parent, i));
                    parent[a] = b;
                }
                under.push(first);
            }
            // No fill inside an outline is a hole; outside every outline, there's no plane. A short stub can lie
            // wholly in a hole, as at a pad: its return current flows in the plane round the hole.
            None if plane.outlines.iter().any(|o| o.contains(p)) => {}
            None => gap = true,
        }
    }
    let roots: Vec<usize> = under.iter().map(|&i| find(&mut parent, i)).collect();
    if roots.iter().any(|r| *r != roots[0]) {
        Reference::Split
    } else if gap {
        Reference::Missing
    } else {
        Reference::Whole
    }
}

fn reference(planes: &[&Plane<'_>], t: &Track) -> Reference {
    // Every quarter millimetre along the track, and its ends.
    let steps = (t.length() / 0.25).ceil().max(1.0) as usize;
    let points: Vec<Point> = (0..=steps)
        .map(|k| {
            let f = k as f64 / steps as f64;
            (
                t.start.0 + f * (t.end.0 - t.start.0),
                t.start.1 + f * (t.end.1 - t.start.1),
            )
        })
        .collect();
    let on: Vec<Reference> = planes.iter().map(|p| on_plane(p, &points)).collect();
    if on.contains(&Reference::Whole) {
        Reference::Whole
    } else if on.contains(&Reference::Split) {
        Reference::Split
    } else {
        Reference::Missing
    }
}

/// Whether a net carries edges: an output pin drives it and no power pin is on it, or it's an SPI or I²S line.
fn carries_edges(design: &Design, net: usize) -> bool {
    let kinds: Vec<PinKind> = design
        .components
        .values()
        .filter(|c| !c.dnp)
        .flat_map(|c| c.pins.values())
        .filter(|p| p.net == net)
        .map(|p| p.kind)
        .collect();
    let driven = kinds.iter().any(|k| {
        matches!(
            k,
            PinKind::Output
                | PinKind::Bidirectional
                | PinKind::TriState
                | PinKind::OpenCollector
                | PinKind::OpenEmitter
        )
    });
    let power = kinds.iter().any(|k| matches!(k, PinKind::PowerIn | PinKind::PowerOut));
    (driven && !power) || is_bus_line(&design.nets[net].name)
}

/// The `plane-split` findings: one per net that carries edges and has tracks.
pub fn check(design: &Design, layout: &Layout) -> Vec<Finding> {
    let layers = layout.copper_layers();
    let planes: Vec<Plane<'_>> = layers.iter().map(|l| Plane::new(layout, l)).collect();
    let neighbours = |layer: &str| -> Vec<&Plane<'_>> {
        let Some(i) = layers.iter().position(|l| *l == layer) else {
            return vec![];
        };
        [i.checked_sub(1), Some(i + 1)]
            .into_iter()
            .flatten()
            .filter_map(|j| planes.get(j))
            .collect()
    };
    let mut out = vec![];
    for (n, net) in design.nets.iter().enumerate() {
        if layout.zone_nets.contains(&net.name) || !carries_edges(design, n) {
            continue;
        }
        let tracks: Vec<&Track> = layout.tracks.iter().filter(|t| t.net == net.name).collect();
        if tracks.is_empty() {
            continue;
        }
        let worst = tracks
            .iter()
            .map(|t| (reference(&neighbours(&t.layer), t), *t))
            .max_by_key(|(r, _)| *r)
            .expect("the net has tracks");
        let at = |t: &Track| {
            format!(
                "the track on {} from ({:.2}, {:.2}) to ({:.2}, {:.2})",
                t.layer, t.start.0, t.start.1, t.end.0, t.end.1
            )
        };
        let (verdict, message) = match worst {
            (Reference::Whole, _) => (
                Verdict::Pass,
                format!(
                    "{}: every track has one unbroken plane beneath it on a neighbouring layer",
                    net.name
                ),
            ),
            (Reference::Split, t) => (
                Verdict::Fail,
                format!(
                    "{}: {} crosses a split: the plane under it is separate pieces and no neighbouring layer \
                     has one unbroken plane under it, so its return current has to go around the gap",
                    net.name,
                    at(t)
                ),
            ),
            (Reference::Missing, t) => (
                Verdict::Unknown,
                format!(
                    "{}: {} has no plane under part of it on any neighbouring layer, so where its return \
                     current flows isn't known",
                    net.name,
                    at(t)
                ),
            ),
        };
        out.push(Finding {
            rule: "plane-split",
            verdict,
            severity: Severity::Warning,
            reference: net.name.clone(),
            message,
            nets: vec![n],
            case: None,
            headline: None,
        });
    }
    out
}
