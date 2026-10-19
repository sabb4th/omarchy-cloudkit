use crate::storage::{Result, text};
use regex::Regex;
use serde_json::Value;
pub fn glob(pattern: &str) -> Result<Regex> {
    let mut chars = pattern.chars().peekable();
    let mut out = if chars.peek() == Some(&'/') {
        chars.next();
        "^".to_owned()
    } else {
        "(^|/)".to_owned()
    };
    let mut braces = 0;
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push_str(&regex::escape(
                &chars
                    .next()
                    .ok_or("Pattern ends with backslash")?
                    .to_string(),
            )),
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    out.push_str(".*")
                } else {
                    out.push_str("[^/]*")
                }
            }
            '?' => out.push_str("[^/]"),
            '[' => {
                out.push('[');
                if chars.peek() == Some(&'!') {
                    chars.next();
                    out.push('^');
                }
                let mut closed = false;
                for c in chars.by_ref() {
                    out.push(c);
                    if c == ']' {
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    return Err("Unclosed [".into());
                }
            }
            '{' => {
                if chars.peek() == Some(&'{') {
                    return Err("Regular expressions are not supported here".into());
                }
                braces += 1;
                out.push('(')
            }
            '}' if braces > 0 => {
                braces -= 1;
                out.push(')')
            }
            ',' if braces > 0 => out.push('|'),
            _ => out.push_str(&regex::escape(&c.to_string())),
        }
    }
    if braces > 0 {
        return Err("Unclosed {".into());
    }
    out.push('$');
    Regex::new(&out).map_err(|e| e.to_string())
}
pub fn clean(values: &Value) -> Result<Vec<String>> {
    let a = values.as_array().ok_or("Patterns must be an array")?;
    let mut out = vec![];
    for v in a {
        let s = text(v)
            .trim()
            .strip_prefix("- ")
            .unwrap_or(text(v).trim())
            .trim();
        if s.is_empty()
            || s.chars().count() > 300
            || s.contains(['\r', '\n', '\0'])
            || s.starts_with("+ ")
        {
            return Err("Invalid never-sync pattern".into());
        }
        glob(s)?;
        if !out.contains(&s.to_owned()) {
            out.push(s.to_owned())
        }
    }
    if out.len() > 500 {
        return Err("At most 500 patterns".into());
    }
    Ok(out)
}
pub fn excludes(raw: &str) -> Vec<String> {
    let mut out = vec![];
    for s in raw.lines().filter_map(|l| l.trim().strip_prefix("- ")) {
        let s = s.trim().to_owned();
        if !s.is_empty() && !out.contains(&s) {
            out.push(s)
        }
    }
    out
}
pub fn others(raw: &str) -> usize {
    raw.lines()
        .filter(|l| {
            let s = l.trim();
            !s.is_empty() && !s.starts_with(['#', ';']) && !s.starts_with("- ")
        })
        .count()
}
pub fn replace(raw: &str, patterns: &[String]) -> String {
    let mut seen = vec![];
    let mut out = String::new();
    for l in raw.lines() {
        if let Some(p) = l.trim().strip_prefix("- ") {
            let p = p.trim().to_owned();
            if !patterns.contains(&p) || seen.contains(&p) {
                continue;
            }
            seen.push(p);
        }
        out.push_str(l);
        out.push('\n');
    }
    for p in patterns {
        if !seen.contains(p) {
            out.push_str(&format!("- {p}\n"));
        }
    }
    out
}
pub fn override_text(p: &[String]) -> String {
    format!(
        "# Written by omarchy-cloudkit; per-folder rules replace global rules.\n{}",
        p.iter().map(|p| format!("- {p}\n")).collect::<String>()
    )
}
pub fn digest(s: &str) -> String {
    format!("{:x}", md5::compute(s))
}
pub struct Matcher(Vec<(bool, Regex)>);
impl Matcher {
    pub fn new(raw: &str) -> Self {
        let mut rules = vec![];
        for l in raw.lines() {
            let s = l.trim();
            if s == "!" {
                rules.clear();
                continue;
            }
            let rule = s
                .strip_prefix("- ")
                .map(|p| (true, p))
                .or_else(|| s.strip_prefix("+ ").map(|p| (false, p)));
            if let Some((exclude, p)) = rule {
                match glob(p.trim()) {
                    Ok(rx) => rules.push((exclude, rx)),
                    Err(_) => break,
                }
            }
        }
        Self(rules)
    }
    fn excluded(&self, p: &str) -> bool {
        self.0
            .iter()
            .find(|(_, rx)| rx.is_match(p))
            .is_some_and(|(b, _)| *b)
    }
    pub fn ignored(&self, p: &str, dir: bool) -> bool {
        for (i, _) in p.match_indices('/') {
            if self.excluded(&p[..=i]) {
                return true;
            }
        }
        self.excluded(&format!("{p}{}", if dir { "/" } else { "" }))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rules_preserve_includes_and_directory_semantics() {
        let raw = "# x\n+ /important.log\n- *.log\n- build/\n";
        let m = Matcher::new(raw);
        assert!(!m.ignored("important.log", false));
        assert!(m.ignored("foo/a.log", false));
        assert!(m.ignored("build/a.rs", false));
        assert!(!m.ignored("a.rs", false));
        assert!(replace(raw, &["*.tmp".into()]).contains("+ /important.log"));
        assert!(glob("{{x}}").is_err());
    }
}
