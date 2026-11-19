//! Comparing a net's worst-case voltage with a pin's limits: each end of the range against each bound, and the
//! verdict and message that come out.
use crate::finding::Verdict;
use crate::limits::{Bound, Range};
use crate::voltage::Span;

/// The result of comparing a voltage with one end of a range.
pub(super) enum End {
    Ok,
    Fail(String),
    Unknown(String),
}

pub(super) fn v(x: f64) -> String {
    format!("{} V", (x * 1000.0).round() / 1000.0)
}

/// Compares the voltage `seen` (the pin from the limit's `to`) with a range. `other` gives the voltage of
/// another pin of the part, from the same `to`, for a bound like `{ pin = "VDD", plus = 0.3 }`; `same` says
/// whether that pin is on the checked pin's net, so the two move together.
/// `floor` says which ends of `seen` are only floors (low, high): a FAIL there becomes UNKNOWN.
pub(super) fn compare(
    seen: Span,
    floor: (bool, bool),
    range: &Range,
    other: &dyn Fn(&str) -> Option<Other>,
) -> Vec<End> {
    let Range::Known { min, max, .. } = range else {
        let Range::Unknown(reason) = range else { unreachable!() };
        return vec![End::Unknown(format!("the datasheet gives no range: {reason}"))];
    };
    let soften = |e: End, f: bool| match e {
        End::Fail(why) if f => End::Unknown(format!(
            "{why}, but that end is only a bound: a regulator isn't held and nothing says how far it sags"
        )),
        e => e,
    };
    vec![
        soften(bound(seen.min, min, other, false), floor.0),
        soften(bound(seen.max, max, other, true), floor.1),
    ]
}

/// Another pin a bound is relative to: its voltage, and how the checked pin relates to it.
pub(super) struct Other {
    pub(super) span: Span,
    /// On the checked pin's net.
    pub(super) same: bool,
    /// The checked pin's net can never rise above it.
    pub(super) below: bool,
}

pub(super) fn bound(seen: Option<f64>, b: &Bound, other: &dyn Fn(&str) -> Option<Other>, is_max: bool) -> End {
    let word = if is_max { "maximum" } else { "minimum" };
    let Some(x) = seen else {
        return match b {
            Bound::Unknown(reason) => End::Unknown(format!(
                "the net's {word} is UNKNOWN, and the datasheet gives none: {reason}"
            )),
            _ => End::Unknown(format!("the net's {word} is UNKNOWN")),
        };
    };
    let beyond = |limit: f64| if is_max { x > limit + 1e-9 } else { x < limit - 1e-9 };
    match b {
        Bound::Unknown(reason) => End::Unknown(format!("the datasheet gives no {word}: {reason}")),
        Bound::Value(limit) => {
            if beyond(*limit) {
                End::Fail(format!("reaches {}, past the {word} of {}", v(x), v(*limit)))
            } else {
                End::Ok
            }
        }
        Bound::Pin { pin, plus, at_most } => {
            let cap = |y: f64| at_most.map_or(y, |c| y.min(c));
            let what = format!(
                "{pin}{}{}",
                if *plus == 0.0 {
                    String::new()
                } else if *plus > 0.0 {
                    format!(" + {plus} V")
                } else {
                    format!(" − {} V", -plus)
                },
                at_most.map(|c| format!(" (at most {})", v(c))).unwrap_or_default()
            );
            let Some(Other { span: r, same, below }) = other(pin) else {
                return End::Unknown(format!("the {word} is {what}, and {pin} isn't connected"));
            };
            if is_max && below && !same && *plus >= 0.0 && r.max.is_some_and(|hi| !beyond(cap(hi + plus))) {
                // Everything that sets this net is bounded by the other pin's net, so it can't rise above it.
                return match at_most {
                    Some(c) if beyond(*c) => End::Fail(format!("reaches {}, past the {word} of {}", v(x), v(*c))),
                    _ => End::Ok,
                };
            }
            if same {
                // The pins share a net, so V(pin) − V(other) is 0 and only `plus` and the cap matter.
                let off = if is_max { *plus >= 0.0 } else { *plus <= 0.0 };
                return match (off, at_most) {
                    (false, _) => End::Fail(format!("the {word} is {what}, and {pin} is on the same net")),
                    (true, Some(c)) if is_max && beyond(*c) => {
                        End::Fail(format!("reaches {}, past the {word} of {}", v(x), v(*c)))
                    }
                    _ => End::Ok,
                };
            }
            // On different nets, the two may not move together: pass only if the worst pairing passes.
            let (Some(lo), Some(hi)) = (r.min, r.max) else {
                return End::Unknown(format!("the {word} is {what}, and {pin}'s range is UNKNOWN"));
            };
            let (strict, loose) = if is_max {
                (cap(lo + plus), cap(hi + plus))
            } else {
                (hi + plus, lo + plus)
            };
            if !beyond(strict) {
                End::Ok
            } else if beyond(loose) {
                End::Fail(format!(
                    "reaches {}, past the {word} of {what}, which is at most {}",
                    v(x),
                    v(loose)
                ))
            } else {
                End::Unknown(format!(
                    "reaches {}; the {word} is {what}, between {} and {}, and nothing says how the two move together",
                    v(x),
                    v(strict.min(loose)),
                    v(strict.max(loose))
                ))
            }
        }
    }
}

/// One verdict from several ends: any FAIL fails, else any UNKNOWN is unknown.
pub(super) fn verdict(ends: &[End]) -> (Verdict, String) {
    let fails: Vec<&str> = ends
        .iter()
        .filter_map(|e| if let End::Fail(s) = e { Some(s.as_str()) } else { None })
        .collect();
    if !fails.is_empty() {
        return (Verdict::Fail, fails.join("; "));
    }
    let unknowns: Vec<&str> = ends
        .iter()
        .filter_map(|e| {
            if let End::Unknown(s) = e {
                Some(s.as_str())
            } else {
                None
            }
        })
        .collect();
    if !unknowns.is_empty() {
        return (Verdict::Unknown, unknowns.join("; "));
    }
    (Verdict::Pass, String::new())
}

/// The size of a voltage of either sign.
pub(super) fn magnitude(s: Span) -> Span {
    match (s.min, s.max) {
        (Some(a), Some(b)) => {
            let hi = a.abs().max(b.abs());
            let lo = if a <= 0.0 && b >= 0.0 {
                0.0
            } else {
                a.abs().min(b.abs())
            };
            Span::new(lo, hi)
        }
        _ => Span::unknown(),
    }
}

/// Whether the end of `s` its magnitude comes from is only a floor.
pub(super) fn magnitude_floor(s: Span, floor: (bool, bool)) -> bool {
    match (s.min, s.max) {
        (Some(lo), Some(hi)) => {
            if hi.abs() >= lo.abs() {
                floor.1
            } else {
                floor.0
            }
        }
        _ => false,
    }
}

/// A FAIL that rests on a floor isn't proven, so it becomes UNKNOWN.
pub(super) fn unless_floor(vd: Verdict, msg: String, on_floor: bool) -> (Verdict, String) {
    if vd == Verdict::Fail && on_floor {
        (
            Verdict::Unknown,
            format!("{msg}; but that end is only a bound: a regulator isn't held and nothing says how far it sags"),
        )
    } else {
        (vd, msg)
    }
}

pub(super) fn value_of(b: &Bound) -> Option<f64> {
    match b {
        Bound::Value(x) => Some(*x),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::Bound;

    fn other(span: Span, same: bool, below: bool) -> impl Fn(&str) -> Option<Other> {
        move |_| Some(Other { span, same, below })
    }

    fn pin_bound(plus: f64, at_most: Option<f64>) -> Bound {
        Bound::Pin {
            pin: "VDD".into(),
            plus,
            at_most,
        }
    }

    fn is(e: &End) -> &'static str {
        match e {
            End::Ok => "ok",
            End::Fail(_) => "fail",
            End::Unknown(_) => "unknown",
        }
    }

    /// `VDD + 0.3 V` with VDD at 3.0 to 3.6 V: 3.2 V always passes, 4.0 V always fails, 3.5 V depends on how
    /// the two move.
    #[test]
    fn relative_bounds() {
        let vdd = other(Span::new(3.0, 3.6), false, false);
        let b = pin_bound(0.3, None);
        assert_eq!(is(&bound(Some(3.2), &b, &vdd, true)), "ok");
        assert_eq!(is(&bound(Some(4.0), &b, &vdd, true)), "fail");
        assert_eq!(is(&bound(Some(3.5), &b, &vdd, true)), "unknown");
        // Bounded by VDD itself: passes even at 3.5 V.
        let tracks = other(Span::new(3.0, 3.6), false, true);
        assert_eq!(is(&bound(Some(3.5), &b, &tracks, true)), "ok");
        // `at_most` caps it: min(VDD + 4, 5.5) fails at 5.6 V even when bounded by VDD.
        let capped = pin_bound(4.0, Some(5.5));
        assert_eq!(is(&bound(Some(5.6), &capped, &tracks, true)), "fail");
        // With VDD from 1.0 V the cap is between 5.0 and 5.5 V.
        let ft = other(Span::new(1.0, 3.6), false, false);
        assert_eq!(is(&bound(Some(5.6), &capped, &ft, true)), "fail");
        assert_eq!(is(&bound(Some(5.2), &capped, &ft, true)), "unknown");
        assert_eq!(is(&bound(Some(4.9), &capped, &ft, true)), "ok");
    }

    /// On the same net, only `plus` matters: `VDD − 0.3 V` as a maximum can never hold.
    #[test]
    fn same_net() {
        let same = other(Span::new(3.0, 3.6), true, true);
        assert_eq!(is(&bound(Some(3.6), &pin_bound(0.3, None), &same, true)), "ok");
        assert_eq!(is(&bound(Some(3.6), &pin_bound(-0.3, None), &same, true)), "fail");
    }

    /// An unknown net against a limit the datasheet doesn't give is UNKNOWN, never a pass.
    #[test]
    fn unknown_against_unknown() {
        let none = |_: &str| None;
        assert_eq!(is(&bound(None, &Bound::Unknown("x".into()), &none, true)), "unknown");
    }

    /// A FAIL on an end that is only a floor becomes UNKNOWN; the other end still fails.
    #[test]
    fn floors_soften_their_own_end() {
        let none = |_: &str| None;
        let range = Range::Known {
            min: Bound::Value(1.7),
            max: Bound::Value(3.3),
            page: Some(1),
            note: None,
            derived: None,
        };
        let ends = compare(Span::new(0.0, 3.5), (true, false), &range, &none);
        assert_eq!(ends.iter().map(is).collect::<Vec<_>>(), ["unknown", "fail"]);
        assert_eq!(verdict(&ends).0, Verdict::Fail);
        let ends = compare(Span::new(0.0, 3.2), (true, false), &range, &none);
        assert_eq!(verdict(&ends).0, Verdict::Unknown);
    }

    /// Magnitude takes its floor from the end it comes from.
    #[test]
    fn magnitude_floors() {
        assert!(!magnitude_floor(Span::new(0.0, 3.3), (true, false)));
        assert!(magnitude_floor(Span::new(-5.0, 3.3), (true, false)));
    }
}
