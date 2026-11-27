//! Terminal control-sequence escaping for human-readable CLI renderers.

use std::fmt::Write as _;

/// Escapes C0, DEL, ESC, C1, and Unicode bidi/line-separator control characters
/// so untrusted telemetry strings cannot inject terminal escape sequences.
#[must_use]
pub fn sanitize_terminal_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x1b' => out.push_str("\\x1b"),
            '\x00'..='\x1f' | '\x7f' => {
                let _ = write!(out, "\\x{:02x}", ch as u32);
            }
            '\u{0080}'..='\u{009f}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}' => {
                let _ = write!(out, "\\u{{{:04x}}}", ch as u32);
            }
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::sanitize_terminal_text;

    #[test]
    fn escapes_c0_del_esc_c1_and_osc_sequences() {
        let evil =
            "ok\x1b[31mred\x1b]0;pwned\x07\x08\x7f\u{009b}2J\u{009d}0;x\u{009c}\r\n\t\u{202e}bidi";
        let safe = sanitize_terminal_text(evil);
        assert_eq!(
            safe,
            "ok\\x1b[31mred\\x1b]0;pwned\\x07\\x08\\x7f\\u{009b}2J\\u{009d}0;x\\u{009c}\\r\\n\\t\\u{202e}bidi"
        );
        assert!(!safe.chars().any(|c| c.is_control()));
    }

    #[test]
    fn preserves_plain_utf8_text() {
        let plain = "app.slice /data/fabrico11y CPU=12.5% ✓";
        assert_eq!(sanitize_terminal_text(plain), plain);
    }
}
