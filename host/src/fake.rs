//! T4a: a fake `Http` for `kio test`. `--http hook=fake:FILE` answers from FILE's entries instead
//! of a server. Only `kio test`'s binder builds one (`Origin::fake`); `Origin::parse` refuses
//! `fake:`. The host's own checks run before a lookup, as on a real grant (`State::prepare`).
use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};

/// One entry of FILE: its line, `METHOD /path`, and the answer.
#[derive(Debug)]
pub struct Entry {
    pub line: usize,
    pub method: &'static str,
    pub path: String,
    answer: Answer,
}

/// What a request gets: a response as written, `timeout`, or no entry (`denied`).
#[derive(Clone, Debug)]
pub(crate) enum Answer {
    /// The body is shared: an answer is not copied until `--io` allows it (`fake_result`).
    Response(u16, Vec<String>, Arc<[u8]>),
    Timeout,
    Unmatched,
}

/// FILE parsed: its entries, in order. Read once, shared by every test.
#[derive(Debug)]
pub struct FakeFile {
    pub entries: Vec<Entry>,
}

/// Parse FILE's `text`; `name` is how errors name it (`hook.http:7: ..`). Entries are separated by
/// a line `---`; each is `METHOD /path`, a status 100-599 or `timeout`, `name: value` headers, a
/// blank line and the body (to the next `---`, its last newline dropped). Blank lines and lines
/// starting with `#` before the request line are skipped.
pub fn parse(name: &str, text: &str) -> Result<FakeFile, String> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    // An empty FILE has no lines, so its error names line 1.
    if text.ends_with('\n') || text.is_empty() {
        lines.pop();
    }
    let mut entries = Vec::new();
    let mut start = 0;
    while start <= lines.len() {
        let end = (start..lines.len())
            .find(|&i| lines[i].trim_end_matches('\r') == "---")
            .unwrap_or(lines.len());
        entries.push(entry(name, &lines[start..end], start + 1)?);
        start = end + 1;
    }
    Ok(FakeFile { entries })
}

/// One entry: `lines`, the first of which is line `first` of FILE.
fn entry(name: &str, lines: &[&str], first: usize) -> Result<Entry, String> {
    let at = |i: usize| format!("{name}:{}", first + i);
    let skip = |l: &&str| l.trim().is_empty() || l.starts_with('#');
    let i = lines.iter().position(|l| !skip(l)).unwrap_or(lines.len());
    let request = lines.get(i).map(|l| l.trim_end_matches('\r'));
    let parsed = request.and_then(|l| l.split_once(' ')).and_then(|(m, p)| {
        let m = lang::HTTP_METHODS.iter().find(|k| **k == m)?;
        // A path no request could carry would never match: refused here.
        crate::request_path(p).then_some((*m, p))
    });
    let Some((method, path)) = parsed else {
        return Err(format!(
            "{}: expected METHOD /path ({})",
            at(i),
            "GET, HEAD, POST, PUT, PATCH, DELETE or OPTIONS"
        ));
    };
    let status = lines.get(i + 1).map(|l| l.trim());
    let answer = match status {
        Some("timeout") => match lines[i + 2..].iter().position(|l| !l.trim().is_empty()) {
            None => Answer::Timeout,
            Some(j) => return Err(format!("{}: expected --- after timeout", at(i + 2 + j))),
        },
        Some(s) => match s.parse::<u16>() {
            Ok(n) if (100..=599).contains(&n) && s.bytes().all(|b| b.is_ascii_digit()) => {
                response(lines, i + 2, n, &at)?
            }
            _ => return Err(format!("{}: expected a status 100-599 or timeout", at(i + 1))),
        },
        None => return Err(format!("{}: expected a status 100-599 or timeout", at(i + 1))),
    };
    Ok(Entry {
        line: first + i,
        method,
        path: path.to_string(),
        answer,
    })
}

/// A response with `status`: the headers from `lines[from..]` to a blank line, then the body.
fn response(lines: &[&str], from: usize, status: u16, at: &dyn Fn(usize) -> String) -> Result<Answer, String> {
    let token = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
    let mut headers = Vec::new();
    let mut i = from;
    while i < lines.len() && !lines[i].trim().is_empty() {
        let header = lines[i]
            .split_once(':')
            .filter(|(n, _)| !n.is_empty() && n.chars().all(token));
        let Some((n, v)) = header else {
            return Err(format!("{}: expected name: value", at(i)));
        };
        headers.push(format!("{}: {}", n.to_ascii_lowercase(), v.trim()));
        i += 1;
    }
    let body: Vec<&str> = (lines.get(i + 1..).unwrap_or_default().iter())
        .map(|l| l.trim_end_matches('\r'))
        .collect();
    Ok(Answer::Response(status, headers, body.join("\n").into_bytes().into()))
}

/// The most unmatched requests `Fake::unmatched` names.
const SHOWN: usize = 5;

/// A fake for one test: FILE's entries, what this test has asked of them, and where
/// `--http-record` writes each request (T4b).
#[derive(Debug)]
pub struct Fake {
    file: Arc<FakeFile>,
    asked: Mutex<Asked>,
    record: Option<Record>,
}

/// `--http-record grant=dir`: the grant's name (a kio identifier, so no path characters) and the
/// open handle of the test's default temporary `Dir` the records go into.
#[derive(Debug)]
pub struct Record {
    pub grant: String,
    pub dir: cap_std::fs::Dir,
}

/// Requests so far: per `METHOD /path` how many, in all how many, and those with no entry.
#[derive(Debug, Default)]
struct Asked {
    counts: HashMap<(&'static str, String), usize>,
    total: usize,
    unmatched: Vec<String>,
}

impl Fake {
    pub fn new(file: Arc<FakeFile>, record: Option<Record>) -> Fake {
        Fake {
            file,
            asked: Mutex::default(),
            record,
        }
    }

    /// Whether requests are recorded (`--http-record`).
    pub(crate) fn records(&self) -> bool {
        self.record.is_some()
    }

    /// The answer to the next `method path`: the n-th entry for it answers the n-th such request
    /// and the last repeats; no entry is `Unmatched`, and kept. With `--http-record`, `request`
    /// is written first as `<grant>-<n>`, n counting this test's requests from 1; an unmatched
    /// request too, since it was sent. A failed write is the error, and the request is not counted.
    pub(crate) fn answer(&self, method: &'static str, path: &str, request: &[u8]) -> Result<Answer, String> {
        let mut asked = self.asked.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(r) = &self.record {
            let name = format!("{}-{}", r.grant, asked.total + 1);
            // A new file only: a record never replaces one, even one the program wrote first.
            let mut opts = cap_std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            let written = (r.dir.open_with(&name, &opts)).and_then(|mut f| f.write_all(request));
            if let Err(e) = written {
                return Err(format!(
                    "--http-record {}: cannot write `{name}`: {e}; a record is a new file, so leave `{}-*` names to the host",
                    r.grant, r.grant
                ));
            }
        }
        asked.total += 1;
        let entries: Vec<&Entry> = (self.file.entries.iter())
            .filter(|e| e.method == method && e.path == path)
            .collect();
        let Some(last) = entries.last() else {
            asked.unmatched.push(format!("{method} {path}"));
            return Ok(Answer::Unmatched);
        };
        let n = asked.counts.entry((method, path.to_string())).or_default();
        let e = entries.get(*n).unwrap_or(last);
        *n += 1;
        Ok(e.answer.clone())
    }

    /// `3 requests, 1 unmatched: POST /v1/evnts` (the first `SHOWN` unmatched), or `None` when
    /// every request had an entry. The paths are the program's: the caller escapes them.
    pub fn unmatched(&self) -> Option<String> {
        let asked = self.asked.lock().unwrap_or_else(PoisonError::into_inner);
        if asked.unmatched.is_empty() {
            return None;
        }
        let requests = if asked.total == 1 { "request" } else { "requests" };
        let more = if asked.unmatched.len() > SHOWN { ", .." } else { "" };
        Some(format!(
            "{} {requests}, {} unmatched: {}{more}",
            asked.total,
            asked.unmatched.len(),
            asked.unmatched[..asked.unmatched.len().min(SHOWN)].join(", ")
        ))
    }
}
