//! Minimal Prometheus text exposition renderer (format 0.0.4), hand-written.
//!
//! Why not `prometheus-client`: the renderer is ~200 lines, needs no global state, and we render from
//! plain snapshots on scrape (nothing registers handles on the hot path). A dependency would add a
//! second counter/atomic model next to `mev-metrics` for no benefit.
//!
//! Safety rules enforced here, not left to callers:
//! * label values are bounded (length cap) and anything address-like or hex-run-like becomes `other`;
//! * at most [`MAX_SERIES_PER_FAMILY`] series per family and [`MAX_BYTES`] per scrape, extra series are dropped
//!   (and counted in [`Exposition::dropped`]) instead of growing;
//! * names are validated; an invalid family or label name drops the sample.

use std::fmt::Write as _;

pub const MAX_SERIES_PER_FAMILY: usize = 64;
pub const MAX_BYTES: usize = 512 * 1024;
const MAX_LABEL_VALUE: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Counter,
    Gauge,
    Histogram,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Counter => "counter",
            Kind::Gauge => "gauge",
            Kind::Histogram => "histogram",
        }
    }
}

#[derive(Debug, Default)]
pub struct Exposition {
    out: String,
    names: Vec<String>,
    /// Current family: `(name, kind, series so far)`; `None` when it was rejected.
    cur: Option<(String, Kind, usize)>,
    dropped: u64,
}

fn valid_metric_name(n: &str) -> bool {
    let mut c = n.chars();
    matches!(c.next(), Some(f) if f.is_ascii_alphabetic() || f == '_' || f == ':')
        && c.all(|x| x.is_ascii_alphanumeric() || x == '_' || x == ':')
}

fn valid_label_name(n: &str) -> bool {
    let mut c = n.chars();
    matches!(c.next(), Some(f) if f.is_ascii_alphabetic() || f == '_')
        && !n.starts_with("__")
        && c.all(|x| x.is_ascii_alphanumeric() || x == '_')
}

/// True for strings that look like an address or hash: a `0x` prefix or a run of 16+ hex digits.
pub fn address_like(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("0x") && lower.len() > 2 {
        return true;
    }
    let mut run = 0usize;
    for ch in lower.chars() {
        if ch.is_ascii_hexdigit() {
            run += 1;
            if run >= 16 {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

/// A label value that is safe to expose: bounded and never address-like.
pub fn bound_label_value(v: &str) -> &str {
    if v.is_empty() || v.len() > MAX_LABEL_VALUE || address_like(v) {
        "other"
    } else {
        v
    }
}

/// Escape per the text format: backslash, double quote, newline.
pub fn escape_label_value(v: &str) -> String {
    let mut s = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => s.push_str("\\\\"),
            '"' => s.push_str("\\\""),
            '\n' => s.push_str("\\n"),
            c => s.push(c),
        }
    }
    s
}

fn escape_help(v: &str) -> String {
    v.replace('\\', "\\\\").replace('\n', "\\n")
}

pub fn fmt_value(v: f64) -> String {
    if v.is_nan() {
        "NaN".into()
    } else if v == f64::INFINITY {
        "+Inf".into()
    } else if v == f64::NEG_INFINITY {
        "-Inf".into()
    } else {
        format!("{v}")
    }
}

impl Exposition {
    pub fn new() -> Self {
        Self::default()
    }

    /// Samples or families dropped by the safety rules.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Family names declared so far, in order.
    pub fn family_names(&self) -> &[String] {
        &self.names
    }

    /// Start a family. A duplicate or invalid name rejects the family: its samples are dropped.
    pub fn family(&mut self, name: &str, help: &str, kind: Kind) {
        if !valid_metric_name(name)
            || self.names.iter().any(|n| n == name)
            || self.out.len() >= MAX_BYTES
        {
            self.cur = None;
            self.dropped += 1;
            return;
        }
        let _ = writeln!(self.out, "# HELP {name} {}", escape_help(help));
        let _ = writeln!(self.out, "# TYPE {name} {}", kind.as_str());
        self.names.push(name.to_string());
        self.cur = Some((name.to_string(), kind, 0));
    }

    fn labels_text(labels: &[(&str, &str)], extra: Option<(&str, &str)>) -> Option<String> {
        let mut s = String::new();
        let all = labels.iter().copied().chain(extra);
        for (i, (k, v)) in all.enumerate() {
            if !valid_label_name(k) {
                return None;
            }
            if i > 0 {
                s.push(',');
            }
            let v = bound_label_value(v);
            let _ = write!(s, "{k}=\"{}\"", escape_label_value(v));
        }
        Some(if s.is_empty() { s } else { format!("{{{s}}}") })
    }

    fn admit(&mut self) -> Option<String> {
        let (name, _, n) = self.cur.as_mut()?;
        if *n >= MAX_SERIES_PER_FAMILY || self.out.len() >= MAX_BYTES {
            self.dropped += 1;
            return None;
        }
        *n += 1;
        Some(name.clone())
    }

    /// One counter or gauge sample of the current family.
    pub fn sample(&mut self, labels: &[(&str, &str)], value: f64) {
        match self.cur {
            Some((_, Kind::Histogram, _)) | None => {
                self.dropped += 1;
                return;
            }
            Some(_) => {}
        }
        let Some(lt) = Self::labels_text(labels, None) else {
            self.dropped += 1;
            return;
        };
        if let Some(name) = self.admit() {
            let _ = writeln!(self.out, "{name}{lt} {}", fmt_value(value));
        }
    }

    pub fn sample_u64(&mut self, labels: &[(&str, &str)], value: u64) {
        self.sample(labels, value as f64);
    }

    /// One histogram series of the current family. `per_bucket` holds the non-cumulative count of each
    /// finite bound plus one trailing `+Inf` slot (`bounds.len() + 1` entries). Rendered cumulative.
    pub fn histogram(
        &mut self,
        labels: &[(&str, &str)],
        bounds: &[f64],
        per_bucket: &[u64],
        sum: f64,
    ) {
        if !matches!(self.cur, Some((_, Kind::Histogram, _)))
            || per_bucket.len() != bounds.len() + 1
            || bounds.windows(2).any(|w| w[0] >= w[1])
        {
            self.dropped += 1;
            return;
        }
        // One histogram series costs bounds+3 lines; check validity of the label names first.
        if Self::labels_text(labels, None).is_none() {
            self.dropped += 1;
            return;
        }
        let Some(name) = self.admit() else { return };
        let mut cum = 0u64;
        for (i, b) in bounds.iter().enumerate() {
            cum = cum.saturating_add(per_bucket[i]);
            let le = fmt_value(*b);
            let lt = Self::labels_text(labels, Some(("le", &le))).unwrap_or_default();
            let _ = writeln!(self.out, "{name}_bucket{lt} {cum}");
        }
        cum = cum.saturating_add(per_bucket[bounds.len()]);
        let lt = Self::labels_text(labels, Some(("le", "+Inf"))).unwrap_or_default();
        let _ = writeln!(self.out, "{name}_bucket{lt} {cum}");
        let plain = Self::labels_text(labels, None).unwrap_or_default();
        let _ = writeln!(self.out, "{name}_sum{plain} {}", fmt_value(sum));
        let _ = writeln!(self.out, "{name}_count{plain} {cum}");
    }

    pub fn finish(self) -> String {
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_values_are_escaped_and_bounded() {
        assert_eq!(escape_label_value("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
        assert_eq!(bound_label_value("p0"), "p0");
        assert_eq!(bound_label_value(""), "other");
        assert_eq!(bound_label_value(&"x".repeat(49)), "other");
        let addr = ["0x", &"ab".repeat(20)].concat();
        assert_eq!(bound_label_value(&addr), "other");
        assert_eq!(bound_label_value(&"ab".repeat(20)), "other");
        assert_eq!(bound_label_value("circuit_open"), "circuit_open");
    }

    #[test]
    fn invalid_names_drop_the_sample_instead_of_emitting_garbage() {
        let mut e = Exposition::new();
        e.family("bad name", "h", Kind::Gauge);
        e.sample(&[], 1.0);
        e.family("ok_name", "h", Kind::Gauge);
        e.sample(&[("bad-label", "x")], 1.0);
        e.sample(&[("__reserved", "x")], 1.0);
        e.sample(&[("good", "x")], 2.0);
        let text = e.finish();
        assert!(!text.contains("bad name"));
        assert!(text.contains("ok_name{good=\"x\"} 2"));
        assert!(!text.contains("bad-label"));
    }

    #[test]
    fn duplicate_family_and_series_cap() {
        let mut e = Exposition::new();
        e.family("dup", "h", Kind::Counter);
        e.family("dup", "h", Kind::Counter);
        e.sample(&[], 1.0);
        assert_eq!(e.family_names(), ["dup"]);
        assert_eq!(e.dropped(), 2);
        let mut e = Exposition::new();
        e.family("capped", "h", Kind::Gauge);
        for i in 0..(MAX_SERIES_PER_FAMILY + 10) {
            e.sample(&[("k", &format!("v{i}"))], 1.0);
        }
        assert_eq!(e.dropped(), 10);
        assert_eq!(
            e.finish()
                .lines()
                .filter(|l| l.starts_with("capped{"))
                .count(),
            MAX_SERIES_PER_FAMILY
        );
    }

    #[test]
    fn histogram_is_cumulative_with_inf_sum_and_count() {
        let mut e = Exposition::new();
        e.family("h_seconds", "h", Kind::Histogram);
        e.histogram(&[("status", "ok")], &[0.1, 1.0], &[2, 3, 1], 4.5);
        let t = e.finish();
        assert!(t.contains("h_seconds_bucket{status=\"ok\",le=\"0.1\"} 2\n"));
        assert!(t.contains("h_seconds_bucket{status=\"ok\",le=\"1\"} 5\n"));
        assert!(t.contains("h_seconds_bucket{status=\"ok\",le=\"+Inf\"} 6\n"));
        assert!(t.contains("h_seconds_sum{status=\"ok\"} 4.5\n"));
        assert!(t.contains("h_seconds_count{status=\"ok\"} 6\n"));
    }

    #[test]
    fn histogram_with_wrong_shape_is_dropped() {
        let mut e = Exposition::new();
        e.family("h", "h", Kind::Histogram);
        e.histogram(&[], &[1.0, 0.5], &[0, 0, 0], 0.0);
        e.histogram(&[], &[1.0], &[0], 0.0);
        assert_eq!(e.dropped(), 2);
    }
}
