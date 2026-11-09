//! Reads the timestamp, severity and JSON fields of a log line, working on bytes so that
//! lines that are not UTF-8 still parse. Finding the severity never allocates.

/// How serious a line says it is, from least to most.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    #[default]
    Unknown,
    /// Debug and trace lines.
    Debug,
    Info,
    Warning,
    /// Error, critical, fatal and panic lines.
    Error,
}

impl Severity {
    /// The word the log dock shows, empty for an unknown severity.
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warning => "WARN",
            Self::Error => "ERROR",
        }
    }

    pub(super) fn to_byte(self) -> u8 {
        self as u8
    }

    pub(super) fn from_byte(byte: u8) -> Self {
        match byte {
            1 => Self::Debug,
            2 => Self::Info,
            3 => Self::Warning,
            4 => Self::Error,
            _ => Self::Unknown,
        }
    }
}

/// Only this much of a plain line is searched for its severity, which keeps the cost
/// of a line bounded. Formats put the level near the start.
const TEXT_WINDOW: usize = 256;
/// The longest timestamp prefix: nanoseconds and a numeric offset.
const MAX_STAMP_BYTES: usize = "2026-11-07T10:42:06.123456789+05:30".len();
const NANOS_PER_SECOND: i64 = 1_000_000_000;

/// Splits the Kubernetes timestamp prefix from a stored line, as nanoseconds since the
/// Unix epoch. A line without one comes back whole.
pub fn split_timestamp(line: &[u8]) -> (Option<i64>, &[u8]) {
    let window = &line[..line.len().min(MAX_STAMP_BYTES + 1)];
    let Some(space) = window.iter().position(|byte| *byte == b' ') else {
        return (None, line);
    };
    match parse_rfc3339(&line[..space]) {
        Some(nanos) => (Some(nanos), &line[space + 1..]),
        None => (None, line),
    }
}

fn digits(bytes: &[u8]) -> Option<i64> {
    if bytes.is_empty() {
        return None;
    }
    bytes.iter().try_fold(0i64, |value, byte| {
        byte.is_ascii_digit()
            .then(|| value * 10 + i64::from(byte - b'0'))
    })
}

/// Parses `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)`, the form the kubelet writes.
pub(super) fn parse_rfc3339(stamp: &[u8]) -> Option<i64> {
    if stamp.len() < "2026-11-07T10:42:06Z".len()
        || stamp[4] != b'-'
        || stamp[7] != b'-'
        || !matches!(stamp[10], b'T' | b't')
        || stamp[13] != b':'
        || stamp[16] != b':'
    {
        return None;
    }
    let year = digits(&stamp[0..4])?;
    let month = digits(&stamp[5..7])?;
    let day = digits(&stamp[8..10])?;
    let hour = digits(&stamp[11..13])?;
    let minute = digits(&stamp[14..16])?;
    let second = digits(&stamp[17..19])?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    // A leap second shows as :60, and lands on the next second.
    if second > 60 {
        return None;
    }
    let mut rest = &stamp[19..];
    let mut fraction = 0;
    if let [b'.', tail @ ..] = rest {
        let count = tail.iter().take_while(|byte| byte.is_ascii_digit()).count();
        if count == 0 {
            return None;
        }
        let mut scale = NANOS_PER_SECOND;
        for byte in &tail[..count.min(9)] {
            scale /= 10;
            fraction += i64::from(byte - b'0') * scale;
        }
        rest = &tail[count..];
    }
    let offset = match rest {
        [b'Z' | b'z'] => 0,
        [sign @ (b'+' | b'-'), hours @ .., b':', _, _] if hours.len() == 2 => {
            let hours = digits(hours)?;
            let minutes = digits(&rest[4..6])?;
            let offset = (hours * 60 + minutes) * 60;
            if *sign == b'-' { -offset } else { offset }
        }
        _ => return None,
    };
    let seconds =
        days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 - offset + second;
    Some(seconds * NANOS_PER_SECOND + fraction)
}

/// Days since 1970-01-01 in the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The severity of a stored line, after its Kubernetes timestamp.
pub fn line_severity(line: &[u8]) -> Severity {
    severity(split_timestamp(line).1)
}

/// The severity a container's log line states, from a JSON level field or a plain format.
pub fn severity(message: &[u8]) -> Severity {
    let trimmed = message.trim_ascii_start();
    if trimmed.first() == Some(&b'{')
        && let Some(severity) = json_severity(trimmed)
    {
        return severity;
    }
    text_severity(message)
}

/// How a word reads as a level, and whether it counts anywhere as a whole uppercase word.
/// Words that are not `bare` count only where a format puts a level, such as `[note]`.
struct Level {
    severity: Severity,
    bare: bool,
}

fn level(word: &[u8]) -> Option<Level> {
    let mut lower = [0u8; 13];
    let lower = lower.get_mut(..word.len())?;
    for (to, from) in lower.iter_mut().zip(word) {
        *to = from.to_ascii_lowercase();
    }
    let (severity, bare) = match &*lower {
        b"error" | b"err" | b"eror" | b"fatal" | b"ftl" | b"critical" | b"crit" | b"crt"
        | b"panic" | b"pnc" | b"dpanic" | b"emerg" | b"emergency" | b"alert" | b"severe" => {
            (Severity::Error, true)
        }
        b"warn" | b"warning" | b"wrn" => (Severity::Warning, true),
        b"info" | b"inf" | b"notice" => (Severity::Info, true),
        b"information" | b"informational" | b"note" | b"log" => (Severity::Info, false),
        b"debug" | b"dbg" | b"trace" | b"trc" | b"verbose" | b"vrb" | b"fine" | b"finer"
        | b"finest" => (Severity::Debug, true),
        _ => return None,
    };
    Some(Level { severity, bare })
}

fn is_level_key(key: &[u8]) -> bool {
    [
        b"level".as_slice(),
        b"lvl",
        b"severity",
        b"levelname",
        b"log.level",
        b"loglevel",
        b"log_level",
        b"@l",
    ]
    .iter()
    .any(|candidate| key.eq_ignore_ascii_case(candidate))
}

/// Bytes past ASCII count as word bytes, so a level word inside a word such as `éERROR`
/// is not read as a level.
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || !byte.is_ascii()
}

fn klog_severity(message: &[u8]) -> Option<Severity> {
    let severity = match message.first()? {
        b'I' => Severity::Info,
        b'W' => Severity::Warning,
        b'E' | b'F' => Severity::Error,
        _ => return None,
    };
    let header = message.get(1..9)?;
    let date_and_hour = header[..4].iter().all(u8::is_ascii_digit)
        && header[4] == b' '
        && header[5..7].iter().all(u8::is_ascii_digit)
        && header[7] == b':';
    date_and_hour.then_some(severity)
}

fn text_severity(message: &[u8]) -> Severity {
    if let Some(severity) = klog_severity(message) {
        return severity;
    }
    let window = &message[..message.len().min(TEXT_WINDOW)];
    let first_word = window
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(window.len());
    let mut at = 0;
    while at < window.len() {
        if !is_word_byte(window[at]) {
            at += 1;
            continue;
        }
        let start = at;
        while at < window.len() && is_word_byte(window[at]) {
            at += 1;
        }
        if at == window.len() && message.get(at).copied().is_some_and(is_word_byte) {
            break;
        }
        let word = &window[start..at];
        let next = window.get(at).copied();
        if next == Some(b'=') && is_level_key(word) {
            if let Some(level) = logfmt_value(&message[at + 1..]) {
                return level;
            }
            continue;
        }
        if !word[0].is_ascii_alphabetic() {
            continue;
        }
        let Some(level) = level(word) else {
            continue;
        };
        let uppercase = word.iter().all(|byte| !byte.is_ascii_lowercase());
        let bracketed = matches!(before(window, start), Some(b'[' | b'<' | b':'))
            && matches!(after(window, at), Some(b']' | b'>'));
        let leads = start == first_word && next == Some(b':');
        if (uppercase && (level.bare || next == Some(b':'))) || bracketed || leads {
            return level.severity;
        }
    }
    Severity::Unknown
}

/// The byte before `at`, skipping spaces.
fn before(window: &[u8], at: usize) -> Option<u8> {
    window[..at]
        .iter()
        .rev()
        .find(|byte| **byte != b' ')
        .copied()
}

/// The byte at or after `at`, skipping spaces.
fn after(window: &[u8], at: usize) -> Option<u8> {
    window[at..].iter().find(|byte| **byte != b' ').copied()
}

fn logfmt_value(value: &[u8]) -> Option<Severity> {
    let value = value.strip_prefix(b"\"").unwrap_or(value);
    let end = value
        .iter()
        .position(|byte| !is_word_byte(*byte))
        .unwrap_or(value.len());
    Some(level(&value[..end])?.severity)
}

/// A JSON value, kept as the bytes it was written as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Value<'a> {
    /// A string, with its quotes and with its escapes undecoded.
    String(&'a [u8]),
    /// A number, literal, object or array.
    Other(&'a [u8]),
}

impl<'a> Value<'a> {
    fn raw(self) -> &'a [u8] {
        match self {
            Self::String(raw) | Self::Other(raw) => raw,
        }
    }
}

/// The members of one JSON object, read in order without decoding or allocating.
///
/// It stops at the first byte that is not valid JSON, and `complete` then stays false.
struct Members<'a> {
    bytes: &'a [u8],
    at: usize,
    started: bool,
    complete: bool,
}

impl<'a> Members<'a> {
    fn new(object: &'a [u8]) -> Option<Self> {
        (object.first() == Some(&b'{')).then_some(Self {
            bytes: object,
            at: 1,
            started: false,
            complete: false,
        })
    }

    fn peek(&mut self) -> Option<u8> {
        while let Some(byte) = self.bytes.get(self.at) {
            if !byte.is_ascii_whitespace() {
                return Some(*byte);
            }
            self.at += 1;
        }
        None
    }

    fn expect(&mut self, wanted: u8) -> Option<()> {
        (self.peek()? == wanted).then(|| self.at += 1)
    }

    fn string(&mut self) -> Option<&'a [u8]> {
        self.peek()?;
        let start = self.at;
        self.expect(b'"')?;
        loop {
            match self.bytes.get(self.at)? {
                b'"' => {
                    self.at += 1;
                    return Some(&self.bytes[start..self.at]);
                }
                b'\\' => self.at += 2,
                _ => self.at += 1,
            }
        }
    }

    fn value(&mut self) -> Option<Value<'a>> {
        let first = self.peek()?;
        let start = self.at;
        match first {
            b'"' => return self.string().map(Value::String),
            b'{' | b'[' => {
                let mut depth = 0usize;
                loop {
                    match self.bytes.get(self.at)? {
                        b'"' => {
                            self.string()?;
                            continue;
                        }
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => {
                            depth -= 1;
                            if depth == 0 {
                                self.at += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    self.at += 1;
                }
            }
            _ => {
                while self.bytes.get(self.at).is_some_and(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'+' | b'.')
                }) {
                    self.at += 1;
                }
                if self.at == start {
                    return None;
                }
            }
        }
        Some(Value::Other(&self.bytes[start..self.at]))
    }

    fn member(&mut self) -> Option<(&'a [u8], Value<'a>)> {
        if self.started {
            if self.peek()? == b'}' {
                self.at += 1;
                self.complete = true;
                return None;
            }
            self.expect(b',')?;
        } else {
            self.started = true;
            if self.peek()? == b'}' {
                self.at += 1;
                self.complete = true;
                return None;
            }
        }
        let key = self.string()?;
        self.expect(b':')?;
        let value = self.value()?;
        Some((&key[1..key.len() - 1], value))
    }

    /// Whether the object closed, with nothing but whitespace after it.
    fn ended_cleanly(&mut self) -> bool {
        self.complete && self.peek().is_none()
    }
}

impl<'a> Iterator for Members<'a> {
    type Item = (&'a [u8], Value<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        if self.complete {
            return None;
        }
        self.member()
    }
}

/// The severity of a JSON object line, or `None` if it is not valid JSON and has no level.
fn json_severity(object: &[u8]) -> Option<Severity> {
    let mut members = Members::new(object)?;
    for (key, value) in members.by_ref() {
        if is_level_key(key) {
            if let Some(severity) = json_level(value) {
                return Some(severity);
            }
        } else if key == b"log"
            && let Value::Other(nested) = value
            && let Some(nested) = Members::new(nested)
        {
            for (key, value) in nested {
                if key == b"level"
                    && let Some(severity) = json_level(value)
                {
                    return Some(severity);
                }
            }
        }
    }
    members.ended_cleanly().then_some(Severity::Unknown)
}

/// A level written as a word, or as a bunyan or pino number.
fn json_level(value: Value) -> Option<Severity> {
    match value {
        Value::String(raw) => level(&raw[1..raw.len() - 1]).map(|level| level.severity),
        Value::Other(raw) => {
            let number = digits(raw)?;
            Some(match number {
                50.. => Severity::Error,
                40.. => Severity::Warning,
                30.. => Severity::Info,
                10.. => Severity::Debug,
                _ => Severity::Unknown,
            })
        }
    }
}

/// A JSON line as the log dock shows it: its message, then its other fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JsonLine {
    pub message: String,
    /// The other fields as `key=value`, in their order. Levels and times are left out,
    /// because the dock shows them in their own columns.
    pub fields: String,
}

const MESSAGE_KEYS: [&[u8]; 3] = [b"msg", b"message", b"@message"];
const TIME_KEYS: [&[u8]; 5] = [b"time", b"ts", b"timestamp", b"@timestamp", b"@t"];

/// The message and remaining fields of a JSON object line, if it is one and has a message.
pub fn json_line(message: &[u8]) -> Option<JsonLine> {
    let mut members = Members::new(message.trim_ascii())?;
    let mut text = None;
    let mut fields = String::new();
    for (key, value) in members.by_ref() {
        if text.is_none() && MESSAGE_KEYS.contains(&key) {
            text = Some(json_text(value));
            continue;
        }
        if is_level_key(key) || TIME_KEYS.contains(&key) {
            continue;
        }
        if !fields.is_empty() {
            fields.push(' ');
        }
        fields.push_str(&String::from_utf8_lossy(key));
        fields.push('=');
        let shown = json_text(value);
        if shown.is_empty() || shown.contains(|char: char| char.is_whitespace() || char == '=') {
            fields.push_str(&String::from_utf8_lossy(value.raw()));
        } else {
            fields.push_str(&shown);
        }
    }
    if !members.ended_cleanly() {
        return None;
    }
    Some(JsonLine {
        message: text?,
        fields,
    })
}

/// A string value decoded, or any other value as written.
fn json_text(value: Value) -> String {
    match value {
        Value::String(raw) => serde_json::from_slice(raw)
            .unwrap_or_else(|_| String::from_utf8_lossy(&raw[1..raw.len() - 1]).into_owned()),
        Value::Other(raw) => String::from_utf8_lossy(raw).into_owned(),
    }
}

/// Lines in the formats the parser reads, for measuring it.
#[cfg(test)]
pub(super) const SAMPLE_LINES: [&[u8]; 8] = [
    b"2026-11-07T10:42:06.118123456Z INFO GET /healthz 200 3ms request_id=7be2c1",
    b"2026-11-07T10:42:06.118123456Z {\"seq\": 81, \"level\": \"WARN\", \"route\": \"/api/orders\", \"status\": 500, \"ms\": 412}",
    b"2026-11-07T10:42:06.118123456Z time=\"2026-11-07T10:42:06Z\" level=error msg=\"no route to host\" service=checkout",
    b"2026-11-07T10:42:06.118123456Z E1107 10:42:06.118123       1 reflector.go:150] failed to list *v1.Pod",
    b"2026-11-07T10:42:06.118123456Z 10.4.3.21 - - [07/Nov/2026:10:42:06 +0000] \"GET /v1/orders/18274 HTTP/1.1\" 200 512 \"-\" \"curl/8.7\"",
    b"2026-11-07T10:42:06.118123456Z {\"ts\":1794048126.118,\"caller\":\"server/handler.go:88\",\"msg\":\"request finished\",\"status\":200,\"level\":\"info\"}",
    b"2026-11-07T10:42:06.118123456Z 2026-11-07 10:42:06.118  WARN 1 --- [nio-8080-exec-1] o.s.web.servlet.PageNotFound : No mapping",
    b"2026-11-07T10:42:06.118123456Z connected to postgres at 10.4.12.9:5432 after 3 attempts",
];

#[cfg(test)]
mod tests {
    use super::*;
    use Severity::{Debug, Error, Info, Unknown, Warning};

    #[test]
    fn finds_the_severity_of_plain_formats() {
        let cases: &[(&str, Severity)] = &[
            (
                "ERROR dial tcp 10.4.12.8:5432: connect: connection refused",
                Error,
            ),
            ("WARN slow query", Warning),
            ("WARNING: disk almost full", Warning),
            ("INFO GET /healthz 200 3ms", Info),
            ("DEBUG cache miss", Debug),
            ("TRACE entering handler", Debug),
            ("FATAL out of memory", Error),
            ("CRITICAL broker unreachable", Error),
            ("panic: runtime error: index out of range", Error),
            // logfmt, as logrus and go-kit write it.
            (
                r#"time="2026-11-07T10:42:06Z" level=error msg="no route""#,
                Error,
            ),
            (r#"ts=2026-11-07T10:42:06Z lvl=warn msg=retrying"#, Warning),
            (r#"level="info" msg=started"#, Info),
            (r#"t=1 severity=DEBUG"#, Debug),
            (r#"msg="ERROR in upstream" level=info"#, Error),
            // Brackets, as nginx, Apache, Elixir, MySQL and Serilog write them.
            (
                "2026/11/07 10:42:06 [error] 7#7: *1 connect() failed",
                Error,
            ),
            (
                "2026/11/07 10:42:06 [warn] 7#7: conflicting server name",
                Warning,
            ),
            (
                "[Sat Nov 07 10:42:06.118 2026] [core:error] [pid 7] AH00037",
                Error,
            ),
            (
                "[Sat Nov 07 10:42:06.118 2026] [mpm_event:notice] [pid 1] resuming",
                Info,
            ),
            ("10:42:06.118 [info] Running Shop.Endpoint", Info),
            (
                "2026-11-07T10:42:06.118Z 0 [Warning] [MY-010068] CA certificate",
                Warning,
            ),
            (
                "2026-11-07T10:42:06.118Z 0 [Note] [MY-010931] ready for connections",
                Info,
            ),
            ("[10:42:06 ERR] Unhandled exception", Error),
            ("[10:42:06 INF] Request finished", Info),
            ("[ WARN] padded level", Warning),
            ("<debug> socket opened", Debug),
            // Go klog and glog.
            (
                "E1107 10:42:06.118123       1 reflector.go:150] failed to list",
                Error,
            ),
            (
                "W1107 10:42:06.118123       1 warnings.go:70] deprecated",
                Warning,
            ),
            ("I1107 10:42:06.118123       1 main.go:40] serving", Info),
            (
                "F1107 10:42:06.118123       1 main.go:41] cannot start",
                Error,
            ),
            // Python's logging module and Ruby's Logger.
            ("ERROR:root:payment failed", Error),
            ("WARNING:urllib3.connectionpool:Retrying", Warning),
            ("E, [2026-11-07T10:42:06.118 #1] ERROR -- : boom", Error),
            ("I, [2026-11-07T10:42:06.118 #1]  INFO -- : ok", Info),
            // Java with logback or log4j, and Spring Boot.
            (
                "10:42:06.118 [main] ERROR c.e.shop.Checkout - charge failed",
                Error,
            ),
            (
                "2026-11-07 10:42:06.118  WARN 1 --- [main] o.s.b.Startup : slow",
                Warning,
            ),
            (
                "2026-11-07 10:42:06,118 SEVERE [org.apache.catalina] failed",
                Error,
            ),
            ("Nov 07, 2026 FINE org.example.Cache hit", Debug),
            // Postgres.
            (
                "2026-11-07 10:42:06.118 UTC [1] LOG:  database system is ready",
                Info,
            ),
            (
                "2026-11-07 10:42:06.118 UTC [9] ERROR:  relation does not exist",
                Error,
            ),
            // A message that opens with its level.
            ("error: could not resolve host", Error),
            ("Warning: Permanently added host", Warning),
            ("npm ERR! code ELIFECYCLE", Error),
        ];
        for (line, expected) in cases {
            assert_eq!(severity(line.as_bytes()), *expected, "{line}");
        }
    }

    #[test]
    fn ignores_level_words_that_are_only_text() {
        let cases = [
            "GET /api/errors 200",
            "NOERROR answer for example.com",
            "ERRORS_TOTAL=0",
            "ERROR_CODE is unset",
            "MIRRORERROR",
            "retrying after an error",
            "Errors were found in 0 files",
            "the error rate is 0.1%",
            "user=info@example.com logged in",
            "msg=error",
            "Information about the build",
            "LOG rotation done",
            "E1107",
            "W123 not klog",
            "",
            "   ",
            "[taffrail: tail ended]",
            "Starting, level unknown",
            "level=",
            "level=loud",
        ];
        for line in cases {
            assert_eq!(severity(line.as_bytes()), Unknown, "{line}");
        }
    }

    #[test]
    fn the_first_level_in_a_line_wins() {
        assert_eq!(severity(b"INFO request failed with ERROR 500"), Info);
        assert_eq!(severity(b"level=warn msg=\"ERROR upstream\""), Warning);
    }

    #[test]
    fn finds_the_severity_of_json_lines() {
        let cases: &[(&str, Severity)] = &[
            (r#"{"level":"error","msg":"charge failed"}"#, Error),
            (r#"{"msg":"charge failed","level":"ERROR"}"#, Error),
            (r#"{"severity":"WARNING","message":"slow"}"#, Warning),
            (r#"{"lvl":"dbug"}"#, Unknown),
            (r#"{"lvl":"debug"}"#, Debug),
            (r#"{"levelname":"CRITICAL","name":"root"}"#, Error),
            (r#"{"Level":"Information"}"#, Info),
            (r#"{"@l":"Warning","@mt":"Disk {Disk} is full"}"#, Warning),
            (r#"{"log.level":"warn","message":"ECS flat"}"#, Warning),
            (
                r#"{"@timestamp":"x","log":{"logger":"app","level":"error"},"message":"ECS"}"#,
                Error,
            ),
            (
                r#"{"log":{"origin":{"file":{"line":7}}},"level":"info"}"#,
                Info,
            ),
            (r#"{"level":"dpanic","msg":"zap"}"#, Error),
            // bunyan and pino.
            (r#"{"level":10,"msg":"trace"}"#, Debug),
            (r#"{"level":20,"msg":"debug"}"#, Debug),
            (r#"{"level":30,"msg":"info"}"#, Info),
            (r#"{"level":40,"msg":"warn"}"#, Warning),
            (r#"{"level":50,"msg":"error"}"#, Error),
            (r#"{"level":60,"msg":"fatal"}"#, Error),
            (r#"{"level":5}"#, Unknown),
            (r#"  {"level" : "warn" , "msg" : "spaced"}  "#, Warning),
            // The level wins over level words in other fields.
            (r#"{"msg":"ERROR in body","level":"info"}"#, Info),
            (r#"{"error":"timeout","level":"info"}"#, Info),
            // A valid object with no level has no severity, whatever its text says.
            (r#"{"msg":"ERROR in body"}"#, Unknown),
            (r#"{}"#, Unknown),
            // Strings that hold braces, quotes and escapes don't end the object early.
            (
                r#"{"msg":"a \"}\" and {[","nested":{"a":[1,{"b":"}"}]},"level":"warn"}"#,
                Warning,
            ),
        ];
        for (line, expected) in cases {
            assert_eq!(severity(line.as_bytes()), *expected, "{line}");
        }
    }

    #[test]
    fn invalid_json_falls_back_to_plain_formats() {
        let cases: &[(&str, Severity)] = &[
            (r#"{"msg": oops ERROR here"#, Error),
            (r#"{not json} WARN"#, Warning),
            (r#"{"msg":"unterminated"#, Unknown),
            (r#"{"a":1}} trailing"#, Unknown),
            (r#"{"a":1} ERROR after the object"#, Error),
            // A level read before the object breaks still counts.
            (r#"{"level":"error","msg":"cut off [line truncated]"#, Error),
        ];
        for (line, expected) in cases {
            assert_eq!(severity(line.as_bytes()), *expected, "{line}");
        }
    }

    #[test]
    fn handles_lines_that_are_not_utf8() {
        assert_eq!(severity(b"\xff\xfe ERROR \xc3"), Error);
        assert_eq!(severity(b"{\"level\":\"warn\",\"msg\":\"\xff\"}"), Warning);
        assert_eq!(severity(b"\xffERROR"), Unknown);
        assert_eq!(severity(b"caf\xc3\xa9ERROR"), Unknown);
        assert_eq!(severity(b"caf\xc3\xa9 ERROR"), Error);
        assert_eq!(severity(&[0x80; 64]), Unknown);
    }

    #[test]
    fn searches_only_the_start_of_a_long_plain_line() {
        let mut early = b"ERROR ".to_vec();
        early.extend(std::iter::repeat_n(b'x', 16 * 1024));
        assert_eq!(severity(&early), Error);

        let mut late = vec![b'x'; TEXT_WINDOW];
        late.extend_from_slice(b" ERROR");
        assert_eq!(severity(&late), Unknown);

        let mut cut = vec![b' '; TEXT_WINDOW - 3];
        cut.extend_from_slice(b"ERROR");
        assert_eq!(
            severity(&cut),
            Unknown,
            "a word cut by the window is not read"
        );
    }

    #[test]
    fn reads_the_level_at_the_end_of_a_long_json_line() {
        let padding = "x".repeat(16 * 1024);
        let line = format!(r#"{{"msg":"{padding}","level":"warn"}}"#);
        assert_eq!(severity(line.as_bytes()), Warning);
    }

    #[test]
    fn reads_the_severity_after_the_kubernetes_timestamp() {
        assert_eq!(
            line_severity(b"2026-11-07T10:42:06.118Z error: boom"),
            Error
        );
        assert_eq!(
            line_severity(b"2026-11-07T10:42:06Z {\"level\":\"warn\"}"),
            Warning
        );
        assert_eq!(
            line_severity(b"E1107 10:42:06.118 1 main.go:1] no stamp"),
            Error
        );
    }

    #[test]
    fn severity_round_trips_through_its_stored_byte() {
        for severity in [Unknown, Debug, Info, Warning, Error] {
            assert_eq!(Severity::from_byte(severity.to_byte()), severity);
        }
        assert_eq!(Severity::from_byte(200), Unknown);
    }

    #[test]
    fn severities_order_from_least_to_most_serious() {
        assert!(Unknown < Debug && Debug < Info && Info < Warning && Warning < Error);
    }

    const NOV_7_2026_10_42_06: i64 = 1_794_048_126;

    #[test]
    fn splits_the_kubernetes_timestamp() {
        let cases: &[(&str, Option<i64>, &str)] = &[
            (
                "2026-11-07T10:42:06.118123456Z starting up",
                Some(NOV_7_2026_10_42_06 * NANOS_PER_SECOND + 118_123_456),
                "starting up",
            ),
            (
                "2026-11-07T10:42:06Z x",
                Some(NOV_7_2026_10_42_06 * NANOS_PER_SECOND),
                "x",
            ),
            (
                "2026-11-07T10:42:06.1Z x",
                Some(NOV_7_2026_10_42_06 * NANOS_PER_SECOND + 100_000_000),
                "x",
            ),
            (
                "2026-11-07T10:42:06.1234567891Z extra digits",
                Some(NOV_7_2026_10_42_06 * NANOS_PER_SECOND + 123_456_789),
                "extra digits",
            ),
            (
                "2026-11-07T16:12:06+05:30 offset",
                Some(NOV_7_2026_10_42_06 * NANOS_PER_SECOND),
                "offset",
            ),
            (
                "2026-11-07T05:42:06-05:00 offset",
                Some(NOV_7_2026_10_42_06 * NANOS_PER_SECOND),
                "offset",
            ),
            ("1970-01-01T00:00:00Z epoch", Some(0), "epoch"),
            (
                "2024-02-29T00:00:00Z leap",
                Some(1_709_164_800 * NANOS_PER_SECOND),
                "leap",
            ),
            (
                "2026-11-07T10:42:06Z ",
                Some(NOV_7_2026_10_42_06 * NANOS_PER_SECOND),
                "",
            ),
        ];
        for (line, nanos, rest) in cases {
            let (parsed, message) = split_timestamp(line.as_bytes());
            assert_eq!(parsed, *nanos, "{line}");
            assert_eq!(message, rest.as_bytes(), "{line}");
        }
    }

    #[test]
    fn keeps_lines_without_a_timestamp_whole() {
        let cases = [
            "plain text",
            "no-spaces",
            "2026-11-07Tést x",
            "2026-11-07 10:42:06 a space, not a T",
            "2026-13-07T10:42:06Z bad month",
            "2026-11-07T24:42:06Z bad hour",
            "2026-11-07T10:42:06 no zone",
            "2026-11-07T10:42:06.Z no fraction digits",
            "2026-11-07T10:42:06+0530 offset without a colon",
            "2026-11-07T10:42:06Zjunk x",
            "[taffrail: tail ended]",
            "",
        ];
        for line in cases {
            assert_eq!(
                split_timestamp(line.as_bytes()),
                (None, line.as_bytes()),
                "{line}"
            );
        }
        assert_eq!(
            split_timestamp(b"\xff\xff x"),
            (None, b"\xff\xff x".as_slice())
        );
    }

    fn json(line: &str) -> Option<JsonLine> {
        json_line(line.as_bytes())
    }

    #[test]
    fn shows_a_json_message_and_its_other_fields() {
        assert_eq!(
            json(
                r#"{"level":"error","ts":1.7e9,"msg":"charge failed","order":18274,"retry":true}"#
            ),
            Some(JsonLine {
                message: "charge failed".into(),
                fields: "order=18274 retry=true".into(),
            })
        );
        assert_eq!(
            json(r#"{"@timestamp":"x","message":"ok","log":{"level":"info"}}"#),
            Some(JsonLine {
                message: "ok".into(),
                fields: r#"log={"level":"info"}"#.into(),
            })
        );
        assert_eq!(
            json(r#"{"@message":"from @message","severity":"INFO"}"#),
            Some(JsonLine {
                message: "from @message".into(),
                fields: String::new(),
            })
        );
    }

    #[test]
    fn decodes_json_strings_and_quotes_values_with_spaces() {
        assert_eq!(
            json(
                r#"{"msg":"line\nbreak \"quoted\" é","user":"ada","path":"/a b","empty":"","eq":"a=b"}"#
            ),
            Some(JsonLine {
                message: "line\nbreak \"quoted\" é".into(),
                fields: r#"user=ada path="/a b" empty="" eq="a=b""#.into(),
            })
        );
    }

    #[test]
    fn the_first_message_key_is_the_message() {
        assert_eq!(
            json(r#"{"message":"first","msg":"second"}"#),
            Some(JsonLine {
                message: "first".into(),
                fields: "msg=second".into(),
            })
        );
    }

    #[test]
    fn json_lines_without_a_message_or_that_are_invalid_show_as_written() {
        for line in [
            r#"{"level":"info","seq":1}"#,
            r#"{"msg":"cut off"#,
            r#"{"msg":"x"} trailing"#,
            "not json",
            "",
            "[1,2]",
        ] {
            assert_eq!(json(line), None, "{line}");
        }
    }

    #[test]
    fn a_json_message_that_is_not_a_string_shows_as_written() {
        assert_eq!(
            json(r#"{"msg":42}"#),
            Some(JsonLine {
                message: "42".into(),
                fields: String::new(),
            })
        );
    }

    #[test]
    fn json_with_invalid_utf8_still_shows() {
        let line = json_line(b"{\"msg\":\"caf\xc3\",\"k\":\"\xff\"}").unwrap();
        assert_eq!(line.message, "caf\u{fffd}");
        assert_eq!(line.fields, "k=\u{fffd}");
    }

    /// Run with `cargo test --release -p taffrail-core parse_speed -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, run in release"]
    fn parse_speed() {
        let lines: Vec<&[u8]> = SAMPLE_LINES
            .iter()
            .copied()
            .cycle()
            .take(1_000_000)
            .collect();
        let bytes: usize = lines.iter().map(|line| line.len()).sum();
        let started = std::time::Instant::now();
        let mut counts = [0usize; 5];
        for line in &lines {
            let (time, _) = split_timestamp(line);
            std::hint::black_box(time);
            counts[line_severity(std::hint::black_box(line)) as usize] += 1;
        }
        let elapsed = started.elapsed();
        println!(
            "parsed {} lines ({:.0} MB) in {:.0} ms: {:.0} ns per line, {:.1} million lines per second, {counts:?}",
            lines.len(),
            bytes as f64 / 1e6,
            elapsed.as_secs_f64() * 1e3,
            elapsed.as_nanos() as f64 / lines.len() as f64,
            lines.len() as f64 / elapsed.as_secs_f64() / 1e6,
        );
        assert_eq!(counts.iter().sum::<usize>(), lines.len());
    }
}
