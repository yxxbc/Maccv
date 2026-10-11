//! Sensitive-content detection and masking.
//!
//! A clipboard history is a log of everything the user has copied, which makes it
//! the single most likely place on the Mac to find a password someone copied out
//! of a password manager. This module is how MacCV avoids keeping one.
//!
//! # Two different jobs
//!
//! **Detection** answers "should this even be recorded?". A password manager
//! marks its pasteboard entries as concealed; those should be skipped entirely,
//! not stored and then hidden. See [`Sensitivity::should_skip_recording`].
//!
//! **Redaction** answers "what may be displayed?". A 16-digit card number is
//! worth keeping a record of — the user does need to know they copied it — but
//! the digits should not sit on screen in a menu bar popover.
//!
//! # What this is not
//!
//! Redaction here is a display and storage measure, not a security boundary. Once
//! content is on the system pasteboard any application can read it; nothing in
//! this module changes that. See [`crate::secure_input`] for the one mechanism
//! that genuinely avoids the pasteboard.

use std::ops::Range;

/// What kind of sensitive content a string appears to contain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum SensitiveKind {
    /// The source explicitly marked the pasteboard entry as concealed.
    ///
    /// Password managers do this, and it is the only signal that is a statement
    /// of intent rather than a guess. It always means "do not record".
    Concealed,
    /// The source marked the entry as transient (never to be stored).
    Transient,
    /// A payment card number that passes the Luhn checksum.
    CardNumber,
    /// A string shaped like an API key, token or secret.
    ApiKey,
    /// An AWS access key identifier.
    AwsAccessKey,
    /// A JWT.
    JsonWebToken,
    /// A PEM-encoded private key.
    PrivateKey,
    /// A `user:password@host` connection string.
    ConnectionString,
    /// An email address.
    Email,
    /// A mainland-China resident identity number (18 digits with a valid checksum).
    ChineseIdNumber,
}

impl SensitiveKind {
    /// Whether content of this kind must never be written to history at all.
    ///
    /// Only explicit source signals qualify. Everything else is a heuristic, and a
    /// heuristic that silently discards the user's clipboard is worse than one
    /// that masks a few characters it did not need to.
    pub const fn should_skip_recording(self) -> bool {
        matches!(self, Self::Concealed | Self::Transient | Self::PrivateKey)
    }

    /// A short identifier, suitable for a settings toggle or a log line.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Concealed => "concealed",
            Self::Transient => "transient",
            Self::CardNumber => "card-number",
            Self::ApiKey => "api-key",
            Self::AwsAccessKey => "aws-access-key",
            Self::JsonWebToken => "json-web-token",
            Self::PrivateKey => "private-key",
            Self::ConnectionString => "connection-string",
            Self::Email => "email",
            Self::ChineseIdNumber => "chinese-id-number",
        }
    }

    /// A human-readable label, for the settings list.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Concealed => "Concealed by the source app",
            Self::Transient => "Marked transient by the source app",
            Self::CardNumber => "Payment card number",
            Self::ApiKey => "API key or token",
            Self::AwsAccessKey => "AWS access key",
            Self::JsonWebToken => "JSON Web Token",
            Self::PrivateKey => "Private key",
            Self::ConnectionString => "Connection string with credentials",
            Self::Email => "Email address",
            Self::ChineseIdNumber => "Chinese resident ID number",
        }
    }
}

/// One detected span within a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensitiveMatch {
    /// What was detected.
    pub kind: SensitiveKind,
    /// The byte range it occupies.
    pub range: Range<usize>,
}

impl SensitiveMatch {
    /// The matched text, given the string it came from.
    pub fn as_str<'a>(&self, source: &'a str) -> &'a str {
        &source[self.range.clone()]
    }
}

/// How much of a sensitive value to leave visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RedactionMode {
    /// Replace the whole value with a fixed marker.
    ///
    /// The safest option, and the default: nothing about the value is recoverable
    /// from the display.
    #[default]
    Full,
    /// Keep the first and last few characters, mask the middle.
    ///
    /// Enough for a user to recognise *which* card or key it was without being
    /// able to use it from a screenshot.
    Partial,
}

/// What redaction produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redacted {
    /// The masked text, safe to display.
    pub text: String,
    /// Every kind that contributed, deduplicated and ordered.
    pub kinds: Vec<SensitiveKind>,
    /// Span count, before deduplication.
    pub match_count: usize,
}

impl Redacted {
    /// Whether anything was masked.
    pub fn is_redacted(&self) -> bool {
        self.match_count > 0
    }

    /// Whether the content should be kept out of history entirely.
    ///
    /// True when any match was an explicit source signal rather than a guess.
    pub fn should_skip_recording(&self) -> bool {
        self.kinds.iter().any(|kind| kind.should_skip_recording())
    }
}

/// The character used to mask a value.
pub const MASK: char = '•';

/// Detects sensitive spans in `text` using every supported heuristic.
///
/// The heuristics are deliberately conservative. A false positive masks
/// characters the user wanted to see; a false negative happens constantly and is
/// unavoidable in a text-based detector, which is exactly why masking is offered
/// alongside — not instead of — not recording concealed entries at all.
pub fn detect(text: &str) -> Vec<SensitiveMatch> {
    let mut found = Vec::new();
    found.extend(detect_aws_access_keys(text));
    found.extend(detect_jwts(text));
    found.extend(detect_pem_private_keys(text));
    found.extend(detect_connection_strings(text));
    found.extend(detect_card_numbers(text));
    found.extend(detect_chinese_id_numbers(text));
    found.extend(detect_email_addresses(text));
    found.extend(detect_generic_api_keys(text));

    // Drop overlaps, keeping the earliest and then the longest match, so a token
    // inside an email address is reported once rather than twice.
    found.sort_by(|a, b| {
        a.range
            .start
            .cmp(&b.range.start)
            .then_with(|| b.range.end.cmp(&a.range.end))
    });
    let mut deduped: Vec<SensitiveMatch> = Vec::with_capacity(found.len());
    for candidate in found {
        if let Some(last) = deduped.last() {
            if candidate.range.start < last.range.end {
                continue;
            }
        }
        deduped.push(candidate);
    }
    deduped
}

/// Masks every sensitive span in `text`.
///
/// `text` is returned unchanged when nothing is detected, so the common case costs
/// one pass of the detectors and no allocation beyond the output.
pub fn redact(text: &str, mode: RedactionMode) -> Redacted {
    let matches = detect(text);
    if matches.is_empty() {
        return Redacted {
            text: text.to_owned(),
            kinds: Vec::new(),
            match_count: 0,
        };
    }

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    let mut kinds = Vec::new();

    for found in &matches {
        out.push_str(&text[cursor..found.range.start]);
        out.push_str(&mask_value(&text[found.range.clone()], mode));
        cursor = found.range.end;
        if !kinds.contains(&found.kind) {
            kinds.push(found.kind);
        }
    }
    out.push_str(&text[cursor..]);

    Redacted {
        text: out,
        kinds,
        match_count: matches.len(),
    }
}

/// Renders one sensitive value in masked form.
fn mask_value(value: &str, mode: RedactionMode) -> String {
    let char_count = value.chars().count();
    match mode {
        RedactionMode::Full => MASK.to_string().repeat(char_count.max(6)),
        RedactionMode::Partial => {
            // Keep a recognisable head and tail. Below 8 characters there is not
            // enough left to be worth revealing anything, so mask it fully.
            if char_count < 8 {
                return MASK.to_string().repeat(char_count.max(6));
            }
            let chars: Vec<char> = value.chars().collect();
            let keep = 3;
            let mut out = String::with_capacity(char_count);
            out.extend(chars[..keep].iter());
            out.extend(std::iter::repeat_n(MASK, char_count - keep * 2));
            out.extend(chars[char_count - keep..].iter());
            out
        }
    }
}

// ---------------------------------------------------------------------------
// Detectors
// ---------------------------------------------------------------------------

/// Separators that appear inside formatted card numbers.
const CARD_SEPARATORS: [char; 3] = [' ', '-', '\u{2011}'];

/// AWS access key IDs: `AKIA`/`ASIA` followed by 16 uppercase alphanumerics.
///
/// Scans bytes rather than slicing the string, because the walk index advances by
/// one byte at a time and would otherwise land inside a multi-byte character. An
/// all-ASCII window is always a valid `&str` range, so accepting only ASCII makes
/// the slice below sound without a boundary check.
fn detect_aws_access_keys(text: &str) -> Vec<SensitiveMatch> {
    /// Four-character prefix plus sixteen characters of body.
    const TOKEN_LEN: usize = 20;

    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut index = 0usize;

    while index + TOKEN_LEN <= bytes.len() {
        let window = &bytes[index..index + TOKEN_LEN];
        let has_prefix = &window[..4] == b"AKIA" || &window[..4] == b"ASIA";
        let body_ok = window[4..]
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());

        if has_prefix && body_ok && is_ascii_word_boundary(bytes, index, index + TOKEN_LEN) {
            found.push(SensitiveMatch {
                kind: SensitiveKind::AwsAccessKey,
                range: index..index + TOKEN_LEN,
            });
            index += TOKEN_LEN;
        } else {
            index += 1;
        }
    }
    found
}

/// JWTs: three base64url segments separated by dots, starting `eyJ` (a JSON
/// header). The signature is optional because truncated tokens are common in logs.
fn detect_jwts(text: &str) -> Vec<SensitiveMatch> {
    let mut found = Vec::new();
    let mut search_from = 0usize;

    while let Some(offset) = text[search_from..].find("eyJ") {
        let start = search_from + offset;
        let rest = &text[start..];

        // Collect the token: base64url plus dots.
        let end_offset = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
            .unwrap_or(rest.len());
        let token = &rest[..end_offset];
        let segments: Vec<&str> = token.split('.').collect();

        // At least header.payload, and the payload must be non-trivially long.
        if segments.len() >= 2 && segments[0].len() >= 8 && segments[1].len() >= 8 {
            found.push(SensitiveMatch {
                kind: SensitiveKind::JsonWebToken,
                range: start..start + end_offset,
            });
            search_from = start + end_offset;
        } else {
            search_from = start + 3;
        }
    }
    found
}

/// PEM blocks. A private key is never something to keep in a history list.
///
/// Matches `-----BEGIN <label>-----`. The label — not the first `-----` — is what
/// distinguishes a private key from a certificate, so the search starts *after*
/// the opening delimiter.
fn detect_pem_private_keys(text: &str) -> Vec<SensitiveMatch> {
    const BEGIN: &str = "-----BEGIN";
    const DASHES: &str = "-----";
    const END: &str = "-----END";
    /// A label containing any of these is a private key.
    const KEY_MARKERS: [&str; 2] = ["PRIVATE KEY", "PGP PRIVATE KEY BLOCK"];

    let mut found = Vec::new();
    let mut search_from = 0usize;

    while let Some(offset) = text[search_from..].find(BEGIN) {
        let start = search_from + offset;
        let after_begin = start + BEGIN.len();

        // The label runs up to the closing dashes of the header line.
        let Some(label_end) = text[after_begin..].find(DASHES).map(|i| after_begin + i) else {
            break;
        };
        let label = &text[after_begin..label_end];
        let header_end = label_end + DASHES.len();

        if !KEY_MARKERS.iter().any(|marker| label.contains(marker)) {
            search_from = header_end;
            continue;
        }

        // Extend through the matching END line when the block is complete.
        let end = text[header_end..]
            .find(END)
            .and_then(|relative| {
                let end_start = header_end + relative;
                text[end_start..]
                    .find(DASHES)
                    .map(|i| end_start + i + DASHES.len())
            })
            .unwrap_or(header_end);

        found.push(SensitiveMatch {
            kind: SensitiveKind::PrivateKey,
            range: start..end,
        });
        search_from = end.max(header_end);
    }
    found
}

/// `scheme://user:password@host` — the password is the sensitive part, but the
/// whole URI is masked because the host pair frequently identifies a system.
fn detect_connection_strings(text: &str) -> Vec<SensitiveMatch> {
    const SCHEMES: [&str; 7] = [
        "postgres://",
        "postgresql://",
        "mysql://",
        "mongodb://",
        "redis://",
        "amqp://",
        "ftp://",
    ];

    let mut found = Vec::new();
    for scheme in SCHEMES {
        let mut search_from = 0usize;
        while let Some(offset) = text[search_from..].find(scheme) {
            let start = search_from + offset;
            let rest = &text[start..];
            let end_offset = rest
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '<' || c == '>')
                .unwrap_or(rest.len());
            let candidate = &rest[..end_offset];

            // Only a URI that actually carries credentials qualifies.
            let authority = candidate.split("//").nth(1).unwrap_or("");
            let authority = authority.split('/').next().unwrap_or("");
            if authority.contains(':') && authority.contains('@') {
                found.push(SensitiveMatch {
                    kind: SensitiveKind::ConnectionString,
                    range: start..start + end_offset,
                });
            }
            search_from = start + end_offset.max(scheme.len());
        }
    }
    found
}

/// Payment card numbers: 13–19 digits, optionally separated, passing Luhn.
///
/// The Luhn check is what makes this usable. Without it, every 16-digit order
/// number and timestamp would be masked.
fn detect_card_numbers(text: &str) -> Vec<SensitiveMatch> {
    let mut found = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0usize;

    while index < chars.len() {
        if !chars[index].is_ascii_digit() {
            index += 1;
            continue;
        }

        // Collect a run of digits and separators.
        let start = index;
        let mut digits = String::new();
        let mut end = index;
        while end < chars.len() {
            let c = chars[end];
            if c.is_ascii_digit() {
                digits.push(c);
                end += 1;
            } else if CARD_SEPARATORS.contains(&c) {
                end += 1;
            } else {
                break;
            }
        }

        // Trim trailing separators, which belong to the surrounding text.
        while end > start && CARD_SEPARATORS.contains(&chars[end - 1]) {
            end -= 1;
        }

        let is_plausible = (13..=19).contains(&digits.len())
            && luhn_valid(&digits)
            && is_word_boundary_chars(&chars, start, end);

        if is_plausible {
            let byte_range = byte_range_of_chars(text, start, end);
            found.push(SensitiveMatch {
                kind: SensitiveKind::CardNumber,
                range: byte_range,
            });
            index = end;
        } else {
            index += 1;
        }
    }
    found
}

/// Luhn checksum, the algorithm every payment card number satisfies.
fn luhn_valid(digits: &str) -> bool {
    let mut sum = 0u32;
    let mut double = false;

    for byte in digits.bytes().rev() {
        let mut value = u32::from(byte - b'0');
        if double {
            value *= 2;
            if value > 9 {
                value -= 9;
            }
        }
        sum += value;
        double = !double;
    }
    sum % 10 == 0
}

/// Whether `yyyymmdd` denotes a date that could be a birth date.
///
/// The checksum alone is weak — roughly one in eleven random 18-digit numbers
/// satisfies it — so the embedded date is validated too. The bounds are the
/// plausible range for a living human rather than the full calendar range.
fn is_plausible_chinese_birth_date(digits: &str) -> bool {
    if digits.len() != 8 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let year: u32 = digits[0..4].parse().unwrap_or(0);
    let month: u32 = digits[4..6].parse().unwrap_or(0);
    let day: u32 = digits[6..8].parse().unwrap_or(0);

    let now_year: u32 = 2026;
    (1900..=now_year).contains(&year) && (1..=12).contains(&month) && (1..=31).contains(&day)
}

/// Mainland-China resident ID numbers: 17 digits plus a check digit.
///
/// The checksum is what distinguishes a real ID from an arbitrary 18-digit
/// number, so an ID-shaped string that fails it is left alone.
fn detect_chinese_id_numbers(text: &str) -> Vec<SensitiveMatch> {
    const WEIGHTS: [u32; 17] = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];
    const CHECK: [char; 11] = ['1', '0', 'X', '9', '8', '7', '6', '5', '4', '3', '2'];
    const LEN: usize = 18;

    let mut found = Vec::new();
    // Walk characters, not bytes: a byte walk would land inside a multi-byte
    // character in any string that is not pure ASCII.
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0usize;

    while index + LEN <= chars.len() {
        let candidate = &chars[index..index + LEN];
        let body_ok = candidate[..17].iter().all(|c| c.is_ascii_digit());
        let last = candidate[17];
        let last_ok = last.is_ascii_digit() || last == 'X';

        let date_ok = {
            let digits: String = candidate[6..14].iter().collect();
            is_plausible_chinese_birth_date(&digits)
        };

        if body_ok && last_ok && date_ok && is_word_boundary_chars(&chars, index, index + LEN) {
            let sum: u32 = candidate[..17]
                .iter()
                .enumerate()
                .map(|(position, c)| (*c as u32 - '0' as u32) * WEIGHTS[position])
                .sum();
            if last == CHECK[(sum % 11) as usize] {
                found.push(SensitiveMatch {
                    kind: SensitiveKind::ChineseIdNumber,
                    range: byte_range_of_chars(text, index, index + LEN),
                });
                index += LEN;
                continue;
            }
        }
        index += 1;
    }
    found
}

/// Email addresses. Kept deliberately simple: a strict RFC 5322 parser would be
/// hundreds of lines and would still not tell us anything more useful.
fn detect_email_addresses(text: &str) -> Vec<SensitiveMatch> {
    let mut found = Vec::new();
    // Character walk: the local part may be preceded by CJK text, and a byte walk
    // would split it mid-character.
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0usize;

    while index < chars.len() {
        if chars[index] != '@' {
            index += 1;
            continue;
        }

        // Walk left over local-part characters.
        let mut start = index;
        while start > 0 {
            let c = chars[start - 1];
            if c.is_ascii_alphanumeric() || "._%+-".contains(c) {
                start -= 1;
            } else {
                break;
            }
        }

        // Walk right over domain characters.
        let mut end = index + 1;
        while end < chars.len() {
            let c = chars[end];
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                end += 1;
            } else {
                break;
            }
        }

        let local: String = chars[start..index].iter().collect();
        let domain: String = chars[index + 1..end].iter().collect();
        let domain_ok = domain.len() >= 4
            && domain.contains('.')
            && !domain.starts_with('.')
            && !domain.ends_with('.')
            && domain
                .rsplit('.')
                .next()
                .is_some_and(|tld| tld.len() >= 2 && tld.bytes().all(|b| b.is_ascii_alphabetic()));

        if !local.is_empty() && domain_ok {
            found.push(SensitiveMatch {
                kind: SensitiveKind::Email,
                range: byte_range_of_chars(text, start, end),
            });
            index = end;
        } else {
            index += 1;
        }
    }
    found
}

/// Prefixes that introduce a secret when followed by a long token.
const SECRET_PREFIXES: [&str; 13] = [
    "sk-", "sk_live_", "sk_test_", "pk_live_", "ghp_", "gho_", "ghu_", "ghs_", "ghr_", "xoxb-",
    "xoxp-", "glpat-", "npm_",
];

/// Generic API keys and tokens: a known prefix followed by a long body.
///
/// This catches the shapes that matter in practice — Stripe, GitHub, Slack,
/// GitLab, npm, and the `sk-` convention shared by several AI providers — without
/// trying to guess at every vendor's format.
fn detect_generic_api_keys(text: &str) -> Vec<SensitiveMatch> {
    /// Minimum body length after the prefix. Below this, the "token" is more
    /// likely an identifier than a secret.
    const MIN_BODY: usize = 16;

    let mut found = Vec::new();
    for prefix in SECRET_PREFIXES {
        let mut search_from = 0usize;
        while let Some(offset) = text[search_from..].find(prefix) {
            let start = search_from + offset;

            // The prefix must start a token, not sit inside a word.
            let at_boundary = start == 0
                || !text[..start]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric());
            if !at_boundary {
                search_from = start + prefix.len();
                continue;
            }

            let rest = &text[start + prefix.len()..];
            let body_len = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
                .unwrap_or(rest.len());

            if body_len >= MIN_BODY {
                found.push(SensitiveMatch {
                    kind: SensitiveKind::ApiKey,
                    range: start..start + prefix.len() + body_len,
                });
                search_from = start + prefix.len() + body_len;
            } else {
                search_from = start + prefix.len();
            }
        }
    }
    found
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Whether `[start, end)` is a whole token rather than a fragment of a larger
/// one, where the indices are **byte** offsets into ASCII-only content.
fn is_ascii_word_boundary(bytes: &[u8], start: usize, end: usize) -> bool {
    let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
    let after_ok = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
    before_ok && after_ok
}

/// Whether `[start, end)` is a whole token, where the indices are **character**
/// offsets. Used by detectors that walk `char`s so they can handle multibyte text.
fn is_word_boundary_chars(chars: &[char], start: usize, end: usize) -> bool {
    let before_ok = start == 0 || !chars[start - 1].is_ascii_alphanumeric();
    let after_ok = end >= chars.len() || !chars[end].is_ascii_alphanumeric();
    before_ok && after_ok
}

/// Byte offsets of the `start`-th and `end`-th characters.
///
/// # Panics
///
/// Never: indices past the end clamp to the string length.
fn byte_range_of_chars(text: &str, start: usize, end: usize) -> Range<usize> {
    let byte_at = |index: usize| {
        text.char_indices()
            .nth(index)
            .map(|(offset, _)| offset)
            .unwrap_or(text.len())
    };
    byte_at(start)..byte_at(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luhn_accepts_real_card_numbers_and_rejects_near_misses() {
        // The canonical Visa/Mastercard test numbers.
        assert!(luhn_valid("4111111111111111"));
        assert!(luhn_valid("5500005555555559"));
        assert!(luhn_valid("378282246310005")); // Amex, 15 digits
        assert!(!luhn_valid("4111111111111112"));
        assert!(!luhn_valid("1234567890123456"));
    }

    #[test]
    fn detects_a_card_number_with_separators() {
        let found = detect("card 4111 1111 1111 1111 end");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, SensitiveKind::CardNumber);
        assert_eq!(
            found[0].as_str("card 4111 1111 1111 1111 end"),
            "4111 1111 1111 1111"
        );
    }

    #[test]
    fn ignores_long_numbers_that_are_not_cards() {
        // Fails Luhn, so it is left alone.
        let found = detect("order 1234567890123456");
        assert!(found.is_empty(), "unexpected: {found:?}");
    }

    #[test]
    fn detects_aws_access_keys() {
        let text = "key AKIAIOSFODNN7EXAMPLE here";
        let found = detect(text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, SensitiveKind::AwsAccessKey);
        assert_eq!(found[0].as_str(text), "AKIAIOSFODNN7EXAMPLE");
    }

    #[test]
    fn ignores_aws_prefix_inside_a_longer_word() {
        let found = detect("XAKIAIOSFODNN7EXAMPLEY");
        assert!(found.is_empty(), "unexpected: {found:?}");
    }

    #[test]
    fn detects_github_and_stripe_tokens() {
        let text = "ghp_NOTAREALKEY and sk_live_NOTAREALKEY";
        let found = detect(text);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().all(|m| m.kind == SensitiveKind::ApiKey));
    }

    #[test]
    fn ignores_short_prefixed_string() {
        // `sk-abc` is far too short to be a secret.
        let found = detect("sk-abc");
        assert!(found.is_empty(), "unexpected: {found:?}");
    }

    #[test]
    fn detects_a_jwt() {
        let text =
            "token eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVP placeholder";
        let found = detect(text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, SensitiveKind::JsonWebToken);
    }

    #[test]
    fn detects_a_private_key_block() {
        let text =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----";
        let found = detect(text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, SensitiveKind::PrivateKey);
        assert!(found[0].kind.should_skip_recording());
    }

    #[test]
    fn does_not_treat_a_public_key_certificate_as_a_private_key() {
        let found = detect("-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----");
        assert!(found.is_empty(), "unexpected: {found:?}");
    }

    #[test]
    fn detects_a_connection_string_with_credentials() {
        let text = "postgres://admin:hunter2@db.internal:5432/app";
        let found = detect(text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, SensitiveKind::ConnectionString);
        assert_eq!(found[0].as_str(text), text);
    }

    #[test]
    fn ignores_a_connection_string_without_credentials() {
        let found = detect("postgres://db.internal:5432/app");
        assert!(found.is_empty(), "unexpected: {found:?}");
    }

    #[test]
    fn detects_a_chinese_id_number_only_with_a_valid_checksum() {
        // Check digit computed from the weighted sum, not guessed.
        let valid = "110101199003077213";
        assert_eq!(detect(valid).len(), 1, "should detect {valid}");

        // Same body, but a check digit that satisfies nothing.
        assert!(detect("110101199003077211").is_empty());

        // Valid checksum, impossible birth month: must not be mislabelled.
        let mut weights = [7u32, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];
        let check = ['1', '0', 'X', '9', '8', '7', '6', '5', '4', '3', '2'];
        let body = "11010119901307721";
        let sum: u32 = body
            .bytes()
            .enumerate()
            .map(|(i, b)| u32::from(b - b'0') * std::mem::take(&mut weights[i]))
            .sum();
        let impossible_month = format!("{body}{}", check[(sum % 11) as usize]);
        assert!(
            detect(&impossible_month).is_empty(),
            "month 13 must not be accepted: {impossible_month}"
        );
    }

    #[test]
    fn detects_email_addresses() {
        let text = "mail me at first.last+tag@example.co.uk please";
        let found = detect(text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, SensitiveKind::Email);
        assert_eq!(found[0].as_str(text), "first.last+tag@example.co.uk");
    }

    #[test]
    fn ignores_things_that_only_look_like_emails() {
        for candidate in ["@example.com", "a@b", "a@.com", "a@com"] {
            assert!(detect(candidate).is_empty(), "should ignore {candidate}");
        }
    }

    #[test]
    fn redact_masks_the_whole_value_by_default() {
        let result = redact("card 4111111111111111", RedactionMode::Full);
        assert!(result.is_redacted());
        assert!(!result.text.contains("4111"));
        assert_eq!(result.kinds, vec![SensitiveKind::CardNumber]);
        assert!(result.text.starts_with("card "));
    }

    #[test]
    fn partial_redaction_keeps_a_recognisable_head_and_tail() {
        let result = redact("4111111111111111", RedactionMode::Partial);
        let masked = &result.text;
        assert!(masked.starts_with("411"));
        assert!(masked.ends_with("111"));
        assert!(masked.contains(MASK));
        // The middle must not survive.
        assert!(!masked.contains("1111111111"));
    }

    #[test]
    fn partial_redaction_fully_masks_short_values() {
        // Eight characters is the threshold; below it nothing is revealed.
        let result = redact("a@b.co", RedactionMode::Partial);
        if result.is_redacted() {
            assert!(!result.text.contains('@'));
        }
    }

    #[test]
    fn redact_leaves_ordinary_text_untouched() {
        let text = "just a normal sentence with no secrets in it";
        let result = redact(text, RedactionMode::Full);
        assert!(!result.is_redacted());
        assert_eq!(result.text, text);
        assert!(result.kinds.is_empty());
    }

    #[test]
    fn redact_handles_multiple_and_overlapping_matches() {
        let text = "email a@b.com and card 4111111111111111";
        let result = redact(text, RedactionMode::Full);
        assert_eq!(result.match_count, 2);
        assert!(result.kinds.contains(&SensitiveKind::Email));
        assert!(result.kinds.contains(&SensitiveKind::CardNumber));
        assert!(!result.text.contains("a@b.com"));
        assert!(!result.text.contains("4111"));
        // The connective text must survive.
        assert!(result.text.contains("email"));
        assert!(result.text.contains("and card"));
    }

    #[test]
    fn matches_never_overlap_and_are_ordered() {
        let text = "ghp_NOTAREALKEY user@example.com";
        let found = detect(text);
        for pair in found.windows(2) {
            assert!(
                pair[0].range.end <= pair[1].range.start,
                "overlap: {found:?}"
            );
        }
    }

    #[test]
    fn redaction_preserves_character_boundaries_for_multibyte_text() {
        let text = "密码 card 4111111111111111 结束";
        let result = redact(text, RedactionMode::Full);
        assert!(result.text.starts_with("密码"));
        assert!(result.text.ends_with("结束"));
        assert!(!result.text.contains("4111"));
    }

    #[test]
    fn sensitive_kinds_report_whether_they_must_be_skipped() {
        assert!(SensitiveKind::Concealed.should_skip_recording());
        assert!(SensitiveKind::Transient.should_skip_recording());
        assert!(SensitiveKind::PrivateKey.should_skip_recording());
        // A guessed match must never cause data loss on its own.
        assert!(!SensitiveKind::CardNumber.should_skip_recording());
        assert!(!SensitiveKind::Email.should_skip_recording());
    }

    #[test]
    fn redacted_reports_skip_when_an_explicit_signal_is_present() {
        let guessed = Redacted {
            text: String::new(),
            kinds: vec![SensitiveKind::CardNumber],
            match_count: 1,
        };
        assert!(!guessed.should_skip_recording());

        let explicit = Redacted {
            text: String::new(),
            kinds: vec![SensitiveKind::Concealed],
            match_count: 1,
        };
        assert!(explicit.should_skip_recording());
    }

    #[test]
    fn every_kind_has_stable_identifiers_and_labels() {
        let kinds = [
            SensitiveKind::Concealed,
            SensitiveKind::Transient,
            SensitiveKind::CardNumber,
            SensitiveKind::ApiKey,
            SensitiveKind::AwsAccessKey,
            SensitiveKind::JsonWebToken,
            SensitiveKind::PrivateKey,
            SensitiveKind::ConnectionString,
            SensitiveKind::Email,
            SensitiveKind::ChineseIdNumber,
        ];
        let mut seen = std::collections::HashSet::new();
        for kind in kinds {
            assert!(
                seen.insert(kind.as_str()),
                "duplicate id: {}",
                kind.as_str()
            );
            assert!(!kind.label().is_empty());
        }
        assert_eq!(seen.len(), kinds.len());
    }

    #[test]
    fn detection_is_cheap_on_large_ordinary_text() {
        // A megabyte of prose must not take perceptible time or allocate much.
        let prose = "The quick brown fox jumps over the lazy dog. ".repeat(24_000);
        let start = std::time::Instant::now();
        let found = detect(&prose);
        let elapsed = start.elapsed();
        assert!(found.is_empty(), "unexpected matches in prose");
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "detection took {elapsed:?} on {} bytes",
            prose.len()
        );
    }
}
