//! Strict validator for Prometheus text format 0.0.4, used by tests (here and in the advisor) to prove
//! what the renderer emits is well formed: HELP/TYPE before samples, label syntax and escapes,
//! no duplicate series, histogram buckets ascending and cumulative with `+Inf`, `_count` equal to `+Inf`,
//! counters finite and non-negative.

use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub name: String,
    pub labels: BTreeMap<String, String>,
    pub value: f64,
}

#[derive(Debug, Default)]
pub struct Parsed {
    /// Family name to TYPE.
    pub families: BTreeMap<String, String>,
    pub samples: Vec<Sample>,
}

impl Parsed {
    pub fn get(&self, name: &str, labels: &[(&str, &str)]) -> Option<f64> {
        self.samples
            .iter()
            .find(|s| {
                s.name == name
                    && labels
                        .iter()
                        .all(|(k, v)| s.labels.get(*k).is_some_and(|x| x == v))
                    && s.labels.len() == labels.len()
            })
            .map(|s| s.value)
    }
}

fn parse_value(v: &str) -> Result<f64, String> {
    match v {
        "NaN" => Ok(f64::NAN),
        "+Inf" => Ok(f64::INFINITY),
        "-Inf" => Ok(f64::NEG_INFINITY),
        _ => v.parse::<f64>().map_err(|_| format!("bad value {v:?}")),
    }
}

fn parse_labels(s: &str) -> Result<(BTreeMap<String, String>, &str), String> {
    // `s` starts after '{'. Returns labels and the rest after '}'.
    let mut out = BTreeMap::new();
    let b = s.as_bytes();
    let mut i = 0;
    loop {
        if i >= b.len() {
            return Err("unterminated labels".into());
        }
        if b[i] == b'}' {
            return Ok((out, &s[i + 1..]));
        }
        let start = i;
        while i < b.len() && b[i] != b'=' {
            i += 1;
        }
        let name = &s[start..i];
        if name.is_empty()
            || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            || name.chars().next().is_some_and(|c| c.is_ascii_digit())
        {
            return Err(format!("bad label name {name:?}"));
        }
        i += 1;
        if b.get(i) != Some(&b'"') {
            return Err("label value must be quoted".into());
        }
        i += 1;
        let mut val = String::new();
        loop {
            match b.get(i) {
                None => return Err("unterminated label value".into()),
                Some(b'"') => {
                    i += 1;
                    break;
                }
                Some(b'\\') => {
                    match b.get(i + 1) {
                        Some(b'\\') => val.push('\\'),
                        Some(b'"') => val.push('"'),
                        Some(b'n') => val.push('\n'),
                        other => return Err(format!("bad escape {other:?}")),
                    }
                    i += 2;
                }
                Some(b'\n') => return Err("raw newline in label value".into()),
                Some(_) => {
                    let ch = s[i..].chars().next().ok_or("utf8")?;
                    val.push(ch);
                    i += ch.len_utf8();
                }
            }
        }
        if out.insert(name.to_string(), val).is_some() {
            return Err(format!("duplicate label {name}"));
        }
        match b.get(i) {
            Some(b',') => i += 1,
            Some(b'}') => {}
            _ => return Err("expected ',' or '}'".into()),
        }
    }
}

pub fn parse_strict(text: &str) -> Result<Parsed, String> {
    if !text.is_empty() && !text.ends_with('\n') {
        return Err("missing trailing newline".into());
    }
    let mut p = Parsed::default();
    let mut helped: BTreeSet<String> = BTreeSet::new();
    let mut seen: BTreeSet<(String, BTreeMap<String, String>)> = BTreeSet::new();
    for (n, line) in text.lines().enumerate() {
        let at = |m: String| format!("line {}: {m}: {line}", n + 1);
        if line.is_empty() {
            return Err(at("empty line".into()));
        }
        if let Some(rest) = line.strip_prefix("# HELP ") {
            let name = rest.split(' ').next().unwrap_or("");
            if !helped.insert(name.to_string()) {
                return Err(at("duplicate HELP".into()));
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("# TYPE ") {
            let mut it = rest.split(' ');
            let (name, kind) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
            if !matches!(kind, "counter" | "gauge" | "histogram") {
                return Err(at(format!("unsupported type {kind:?}")));
            }
            if !helped.contains(name) {
                return Err(at("TYPE without preceding HELP".into()));
            }
            if p.families
                .insert(name.to_string(), kind.to_string())
                .is_some()
            {
                return Err(at("duplicate TYPE".into()));
            }
            continue;
        }
        if line.starts_with('#') {
            return Err(at("unexpected comment".into()));
        }
        let name_end = line
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':'))
            .ok_or_else(|| at("no value".into()))?;
        let name = &line[..name_end];
        let rest = &line[name_end..];
        let (labels, rest) = if let Some(r) = rest.strip_prefix('{') {
            parse_labels(r).map_err(at)?
        } else {
            (BTreeMap::new(), rest)
        };
        let value = rest
            .strip_prefix(' ')
            .ok_or_else(|| at("expected space before value".into()))?;
        if value.contains(' ') {
            return Err(at("timestamps are not emitted".into()));
        }
        let value = parse_value(value).map_err(at)?;
        let fam = if p.families.contains_key(name) {
            name.to_string()
        } else {
            ["_bucket", "_sum", "_count"]
                .iter()
                .find_map(|suf| name.strip_suffix(suf))
                .filter(|f| p.families.get(*f).is_some_and(|k| k == "histogram"))
                .map(str::to_string)
                .ok_or_else(|| at("sample without TYPE".into()))?
        };
        let kind = &p.families[&fam];
        if kind == "counter" && !(value.is_finite() && value >= 0.0) {
            return Err(at("counter must be finite and >= 0".into()));
        }
        if kind != "histogram" && name != fam {
            return Err(at("suffix on non-histogram".into()));
        }
        if !seen.insert((name.to_string(), labels.clone())) {
            return Err(at("duplicate series".into()));
        }
        p.samples.push(Sample {
            name: name.to_string(),
            labels,
            value,
        });
    }
    if let Some(f) = helped.iter().find(|h| !p.families.contains_key(*h)) {
        return Err(format!("HELP without TYPE for {f}"));
    }
    check_histograms(&p)?;
    Ok(p)
}

fn check_histograms(p: &Parsed) -> Result<(), String> {
    for (fam, kind) in &p.families {
        if kind != "histogram" {
            continue;
        }
        // Group by label set without `le`.
        let mut groups: BTreeMap<BTreeMap<String, String>, Vec<&Sample>> = BTreeMap::new();
        for s in p
            .samples
            .iter()
            .filter(|s| s.name.starts_with(fam.as_str()))
        {
            let mut l = s.labels.clone();
            l.remove("le");
            groups.entry(l).or_default().push(s);
        }
        for (labels, ss) in groups {
            let bucket = format!("{fam}_bucket");
            let mut prev_le = f64::NEG_INFINITY;
            let mut prev = 0.0;
            let mut inf = None;
            for s in ss.iter().filter(|s| s.name == bucket) {
                let le = parse_value(s.labels.get("le").ok_or("bucket without le")?)?;
                if le <= prev_le {
                    return Err(format!("{fam}{labels:?}: le not strictly increasing"));
                }
                if s.value < prev {
                    return Err(format!("{fam}{labels:?}: buckets not cumulative"));
                }
                (prev_le, prev) = (le, s.value);
                if le == f64::INFINITY {
                    inf = Some(s.value);
                }
            }
            let inf = inf.ok_or_else(|| format!("{fam}{labels:?}: no +Inf bucket"))?;
            let count = ss
                .iter()
                .find(|s| s.name == format!("{fam}_count"))
                .ok_or_else(|| format!("{fam}{labels:?}: no _count"))?;
            if count.value != inf {
                return Err(format!("{fam}{labels:?}: _count != +Inf bucket"));
            }
            if !ss.iter().any(|s| s.name == format!("{fam}_sum")) {
                return Err(format!("{fam}{labels:?}: no _sum"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_and_rejects_malformed() {
        let ok = "# HELP a_total x\n# TYPE a_total counter\na_total{k=\"v\\\"q\"} 3\n";
        let p = parse_strict(ok).unwrap();
        assert_eq!(p.get("a_total", &[("k", "v\"q")]), Some(3.0));
        for bad in [
            "a 1\n",
            "# HELP a x\n# TYPE a counter\na -1\n",
            "# HELP a x\n# TYPE a gauge\na{k=v} 1\n",
            "# HELP a x\n# TYPE a gauge\na 1\na 2\n",
            "# HELP a x\n# TYPE a gauge\na 1",
            "# TYPE a gauge\na 1\n",
            "# HELP h x\n# TYPE h histogram\nh_bucket{le=\"1\"} 2\nh_bucket{le=\"+Inf\"} 1\nh_sum 1\nh_count 1\n",
            "# HELP h x\n# TYPE h histogram\nh_bucket{le=\"1\"} 1\nh_sum 1\nh_count 1\n",
            "# HELP h x\n# TYPE h histogram\nh_bucket{le=\"1\"} 1\nh_bucket{le=\"+Inf\"} 2\nh_sum 1\nh_count 1\n",
        ] {
            assert!(parse_strict(bad).is_err(), "{bad}");
        }
    }
}
