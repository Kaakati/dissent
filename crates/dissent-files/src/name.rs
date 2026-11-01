//! Sanitizing names and MIME types that arrive from other members.
//!
//! A `FileRef` is written by whoever shared the file, so its name is untrusted input. Callers
//! must never build a path from the raw name. Use [`sanitize_name`] and join the result onto a
//! directory the caller chose.

/// Longest name we keep, in characters.
pub const MAX_NAME_LEN: usize = 128;
/// Longest MIME type we keep, in bytes.
pub const MAX_MIME_LEN: usize = 127;
const FALLBACK_NAME: &str = "file";
const FALLBACK_MIME: &str = "application/octet-stream";

/// Turns a sender's file name into a single safe path component.
///
/// Keeps only the last path segment, drops control, format and bidi characters, replaces
/// characters Windows forbids, and avoids reserved device names. Never returns an empty string
/// or a name that starts or ends with a dot or space.
pub fn sanitize_name(raw: &str) -> String {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let mut cleaned = String::with_capacity(last.len());
    for c in last.chars() {
        if c.is_control() || is_invisible_or_bidi(c) {
            continue;
        }
        cleaned.push(match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            other => other,
        });
    }
    let trimmed: String = cleaned
        .trim()
        .trim_start_matches([' ', '.'])
        .trim_end_matches([' ', '.'])
        .chars()
        .take(MAX_NAME_LEN)
        .collect();
    let trimmed = trimmed.trim_end_matches([' ', '.']);
    if trimmed.is_empty() {
        return FALLBACK_NAME.to_owned();
    }
    if is_reserved_device(trimmed) {
        return format!("_{trimmed}");
    }
    trimmed.to_owned()
}

/// Keeps a MIME type made of token characters and a single slash, or falls back to a generic type.
pub fn sanitize_mime(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "!#$&^_.+-/".contains(*c))
        .take(MAX_MIME_LEN)
        .collect();
    let slashes = cleaned.matches('/').count();
    if slashes == 1 && !cleaned.starts_with('/') && !cleaned.ends_with('/') {
        cleaned
    } else {
        FALLBACK_MIME.to_owned()
    }
}

fn is_invisible_or_bidi(c: char) -> bool {
    matches!(
        c as u32,
        0x00AD | 0x034F | 0x061C | 0x180E | 0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB | 0xE0000..=0xE007F
    )
}

/// Windows reads these as devices, with any extension and with spaces before the dot. That includes COM and LPT with
/// a zero or a superscript digit, and the console's own two names.
fn is_reserved_device(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end().to_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") {
        return true;
    }
    let digit = stem.strip_prefix("COM").or_else(|| stem.strip_prefix("LPT"));
    digit.is_some_and(|d| matches!(d, "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "\u{B9}" | "\u{B2}" | "\u{B3}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_is_neutralised() {
        assert_eq!(sanitize_name("../../evil"), "evil");
        assert_eq!(sanitize_name("..\\..\\evil.exe"), "evil.exe");
        assert_eq!(sanitize_name("C:\\x"), "x");
        assert_eq!(sanitize_name("/etc/passwd"), "passwd");
        assert_eq!(sanitize_name(".."), "file");
        assert_eq!(sanitize_name("."), "file");
    }

    #[test]
    fn output_is_one_plain_component() {
        for raw in ["a/b\\c", "x:stream", "a<b>c|d?e*f\"g", "C:", "\u{0}nul"] {
            let s = sanitize_name(raw);
            assert!(
                !s.contains('/') && !s.contains('\\') && !s.contains(':'),
                "{raw:?} -> {s:?}"
            );
        }
    }

    #[test]
    fn bidi_and_control_characters_are_removed() {
        assert_eq!(sanitize_name("a\u{202E}cod.exe"), "acod.exe");
        assert_eq!(sanitize_name("bad\u{7}name\u{200B}"), "badname");
        assert_eq!(sanitize_name("line\nbreak"), "linebreak");
        assert_eq!(sanitize_name("a\u{AD}b\u{E0041}.exe"), "ab.exe");
    }

    #[test]
    fn reserved_device_names_are_prefixed() {
        assert_eq!(sanitize_name("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_name("lpt1"), "_lpt1");
        assert_eq!(sanitize_name("console.txt"), "console.txt");
        assert_eq!(sanitize_name("NUL .txt"), "_NUL .txt");
        assert_eq!(sanitize_name("com0.log"), "_com0.log");
        assert_eq!(sanitize_name("COM\u{B9}.txt"), "_COM\u{B9}.txt");
        assert_eq!(sanitize_name("lpt\u{B3}"), "_lpt\u{B3}");
        assert_eq!(sanitize_name("CONIN$"), "_CONIN$");
        assert_eq!(sanitize_name("com10.txt"), "com10.txt");
    }

    #[test]
    fn names_are_capped_and_never_empty() {
        let long = "a".repeat(500);
        assert_eq!(sanitize_name(&long).chars().count(), MAX_NAME_LEN);
        assert_eq!(sanitize_name(""), "file");
        assert_eq!(sanitize_name("   "), "file");
        assert_eq!(sanitize_name(".hidden. "), "hidden");
        assert_eq!(sanitize_name(". .hidden"), "hidden");
    }

    #[test]
    fn mime_is_restricted() {
        assert_eq!(sanitize_mime("image/png"), "image/png");
        assert_eq!(
            sanitize_mime("text/plain; charset=utf-8"),
            "text/plaincharsetutf-8"
        );
        assert_eq!(sanitize_mime("no-slash"), "application/octet-stream");
        assert_eq!(sanitize_mime("a/b/c"), "application/octet-stream");
        assert_eq!(sanitize_mime(""), "application/octet-stream");
    }
}
