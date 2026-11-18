use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;

use crate::design::{Design, PinKind};
use crate::inputs::BoardInputs;
use crate::limits::Library;
use crate::thermal::Thermal;
use crate::voltage::{Driver, Hold, Span, Voltages, is_connector};

#[derive(Debug, Clone, PartialEq)]
pub struct Stage {
    pub input: String,
    pub net: usize,
    pub connect_pin: String,
    pub connect: String,
    pub min: f64,
    pub max: f64,
    pub draw: Option<(f64, bool)>,
    pub not_counted: Vec<String>,
    pub rails: Vec<Rail>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rail {
    pub net: usize,
    pub span: Span,
    pub probe: String,
    pub from: Vec<usize>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan {
    pub stages: Vec<Stage>,
    pub ground: Option<String>,
    pub skipped: Vec<String>,
}

pub fn plan(design: &Design, lib: &Library, inputs: &BoardInputs, v: &Voltages, th: &Thermal) -> Plan {
    let net_of = |r: &str, p: &str| design.components.get(r)?.pins.get(p).map(|p| p.net);
    let ground = inputs
        .inputs
        .iter()
        .filter(|i| i.fault.is_none())
        .find_map(|i| net_of(&i.to.reference, &i.to.pin))
        .map(|n| probe(design, lib, n));
    let regulated: BTreeSet<usize> = v
        .regulators
        .iter()
        .filter(|r| r.sets_output)
        .map(|r| r.output)
        .collect();
    let is_rail = |n: usize| {
        let nv = &v.nets[n];
        nv.driver == Driver::Strong
            && nv.span != Span::exact(0.0)
            && (regulated.contains(&n)
                || design.nets[n].pins.iter().any(|(r, p)| {
                    design
                        .components
                        .get(r)
                        .and_then(|c| c.pins.get(p))
                        .is_some_and(|q| q.kind == PinKind::PowerIn)
                }))
    };
    let mut seen: BTreeSet<usize> = BTreeSet::new();
    let mut stages = vec![];
    let mut skipped = vec![];
    for i in inputs.inputs.iter().filter(|i| i.fault.is_none()) {
        let (Some(pn), Some(_)) = (net_of(&i.pin.reference, &i.pin.pin), net_of(&i.to.reference, &i.to.pin)) else {
            continue;
        };
        let crate::inputs::Max::Value(max) = i.max else {
            skipped.push(format!(
                "{}: its maximum follows another pin, so it isn't a bench supply setting",
                i.name
            ));
            continue;
        };
        if !(i.min > 0.0 || max < 0.0) {
            if i.min < 0.0 || max > 0.0 {
                skipped.push(format!(
                    "{}: its range ({} to {max} V) reaches 0 V, so it isn't taken as a supply",
                    i.name, i.min
                ));
            }
            continue;
        }
        let mut rails = vec![];
        let mut queue = VecDeque::from([pn]);
        let mut local = BTreeSet::from([pn]);
        while let Some(n) = queue.pop_front() {
            for m in 0..design.nets.len() {
                if local.contains(&m) || seen.contains(&m) || v.nets[m].driver != Driver::Strong {
                    continue;
                }
                let from: Vec<usize> = v.nets[m].why.iter().flat_map(|w| w.from.iter().copied()).collect();
                if !from.contains(&n) {
                    continue;
                }
                local.insert(m);
                queue.push_back(m);
                if !is_rail(m) {
                    continue;
                }
                let warning = v
                    .regulators
                    .iter()
                    .filter(|r| r.output == m && r.sets_output)
                    .find_map(|r| match &r.hold {
                        Hold::Held(_) => None,
                        Hold::NotHeld(why) => Some(format!("{} may not hold it: {why}", r.reference)),
                        Hold::Unknown(why) => Some(format!("whether {} holds it isn't known: {why}", r.reference)),
                    });
                rails.push(Rail {
                    net: m,
                    span: v.nets[m].span,
                    probe: probe(design, lib, m),
                    from: from
                        .into_iter()
                        .filter(|f| local.contains(f))
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect(),
                    warning,
                });
            }
        }
        seen.extend(local.iter().copied());
        let load = &th.loads[pn];
        let draw = (!load.parts.is_empty() || !load.missing.is_empty()).then_some((load.amps, load.known()));
        stages.push(Stage {
            input: i.name.clone(),
            net: pn,
            connect_pin: i.pin.to_string(),
            connect: format!("{} (+) to {} (−)", i.pin, i.to),
            min: i.min,
            max,
            draw,
            not_counted: load.missing.clone(),
            rails,
        });
    }
    Plan {
        stages,
        ground,
        skipped,
    }
}

fn probe(design: &Design, lib: &Library, net: usize) -> String {
    let pins = &design.nets[net].pins;
    let tp = pins.iter().find(|(r, _)| {
        let letters: String = r.chars().take_while(char::is_ascii_alphabetic).collect();
        letters == "TP"
    });
    let conn = pins
        .iter()
        .find(|(r, _)| design.components.get(r).is_some_and(|c| is_connector(lib, c)));
    match (tp, conn.or(pins.first())) {
        (Some((r, _)), _) => format!("test point {r}"),
        (None, Some((r, p))) => format!("{r}.{p}"),
        (None, None) => design.nets[net].name.clone(),
    }
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn amps(a: f64) -> String {
    if a >= 1.0 {
        format!("{} A", round(a))
    } else {
        format!("{} mA", round(a * 1e3))
    }
}

pub fn render(design: &Design, p: &Plan) -> String {
    let mut out = String::new();
    let name = |n: usize| design.nets[n].name.as_str();
    if p.stages.is_empty() {
        let _ = writeln!(
            out,
            "No supply inputs: inputs.toml has no [[input]] whose range stays above or below 0 V, so there is nothing \
             to power up."
        );
        for s in &p.skipped {
            let _ = writeln!(out, "   - {s}");
        }
        return out;
    }
    let mut step = 1;
    let gnd = p.ground.clone().unwrap_or_else(|| "ground".into());
    let all: Vec<&Rail> = p.stages.iter().flat_map(|s| &s.rails).collect();
    let _ = writeln!(out, "Bring-up plan\n");
    let _ = writeln!(
        out,
        "{step}. With nothing connected, measure the resistance from each rail to ground ({gnd}). A reading near 0 Ω \
         is a short: find it before powering up."
    );
    for s in &p.stages {
        let _ = writeln!(out, "   - {} at {}", name(s.net), s.connect_pin);
    }
    for r in &all {
        let _ = writeln!(out, "   - {} at {}", name(r.net), r.probe);
    }
    step += 1;
    for s in &p.stages {
        let _ = writeln!(out);
        let limit = match s.draw {
            Some((a, true)) => format!(
                "the board's parts draw at most {} from it, so set the current limit a little above that",
                amps(a)
            ),
            Some((a, false)) => format!(
                "the parts that give a current draw at most {} from it, but {} not counted ({}), so start with a low \
                 current limit and raise it while watching the draw",
                amps(a),
                if s.not_counted.len() == 1 {
                    "one part is"
                } else {
                    "some parts are"
                },
                s.not_counted.join("; ")
            ),
            None => "what the board draws from it isn't known, so start with a low current limit and raise it while \
                     watching the draw"
                .into(),
        };
        let _ = writeln!(
            out,
            "{step}. Power {}: connect a bench supply to {}, set inside {} to {} V. Then {limit}.",
            s.input, s.connect, s.min, s.max
        );
        step += 1;
        if s.rails.is_empty() {
            let _ = writeln!(out, "   No rail is worked out from it.");
        }
        for r in &s.rails {
            let from: Vec<&str> = r.from.iter().map(|&f| name(f)).collect();
            let fed = if from.is_empty() {
                String::new()
            } else {
                format!(" (from {})", from.join(", "))
            };
            let _ = writeln!(
                out,
                "{step}. Measure {}{fed} at {}: expect {}.",
                name(r.net),
                r.probe,
                r.span
            );
            if let Some(w) = &r.warning {
                let _ = writeln!(out, "   note: {w}");
            }
            step += 1;
        }
    }
    if !p.skipped.is_empty() {
        let _ = writeln!(out, "\nLeft out of the plan:");
        for s in &p.skipped {
            let _ = writeln!(out, "   - {s}");
        }
    }
    if p.stages.len() > 1 {
        let _ = writeln!(
            out,
            "\nPower one input at a time, in this order, before connecting them together."
        );
    }
    let _ = writeln!(
        out,
        "\nIf a rail is far outside its range or the supply hits its current limit, switch off and find out why \
         before going on."
    );
    out
}

pub fn rails_by_name<'a>(design: &'a Design, p: &Plan) -> BTreeMap<&'a str, Span> {
    p.stages
        .iter()
        .flat_map(|s| &s.rails)
        .map(|r| (design.nets[r.net].name.as_str(), r.span))
        .collect()
}
