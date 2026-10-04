//! Raw keyboard/mouse bytes from the real terminal.
//!
//! In P0 ShellView forwards input to the shell byte-for-byte (so every key,
//! paste and IME sequence works exactly as before) and only picks out:
//! - its own global keys,
//! - mouse-wheel events, when ShellView (not the program inside) owns the mouse.

/// What a chunk of input means to ShellView.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Leave ShellView (ctrl+q).
    Quit,
    /// Full ⇄ basic view (alt+`).
    ToggleView,
    /// Scroll history by N lines (positive = back in time).
    Scroll(isize),
    /// Bytes for the shell.
    Forward(Vec<u8>),
}

const CTRL_Q: u8 = 0x11;
const PASTE_START: &[u8] = b"\x1b[200~";

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Split one read from stdin into actions.
///
/// `own_mouse`: ShellView enabled SGR mouse reporting itself, so mouse
/// sequences are ShellView's to interpret and must not reach the shell.
pub fn parse(chunk: &[u8], own_mouse: bool, page: isize) -> Vec<Action> {
    // Pasted text is never interpreted, whatever it contains.
    if contains(chunk, PASTE_START) {
        return vec![Action::Forward(chunk.to_vec())];
    }
    match chunk {
        [CTRL_Q] => return vec![Action::Quit],
        b"\x1b`" => return vec![Action::ToggleView],
        b"\x1b[5;2~" => return vec![Action::Scroll(page)], // shift+PageUp
        b"\x1b[6;2~" => return vec![Action::Scroll(-page)], // shift+PageDown
        _ => {}
    }

    let mut out = Vec::new();
    let mut fwd = Vec::new();
    let mut i = 0;
    while i < chunk.len() {
        if chunk[i] == CTRL_Q {
            flush(&mut out, &mut fwd);
            out.push(Action::Quit);
            return out;
        }
        if own_mouse && chunk[i..].starts_with(b"\x1b[<") {
            if let Some((len, button)) = sgr_mouse(&chunk[i..]) {
                match button {
                    64 => {
                        flush(&mut out, &mut fwd);
                        out.push(Action::Scroll(3));
                    }
                    65 => {
                        flush(&mut out, &mut fwd);
                        out.push(Action::Scroll(-3));
                    }
                    _ => {} // clicks are ShellView's (unused in P0): swallow
                }
                i += len;
                continue;
            }
        }
        fwd.push(chunk[i]);
        i += 1;
    }
    flush(&mut out, &mut fwd);
    out
}

fn flush(out: &mut Vec<Action>, fwd: &mut Vec<u8>) {
    if !fwd.is_empty() {
        out.push(Action::Forward(std::mem::take(fwd)));
    }
}

/// Parse `ESC [ < button ; x ; y (M|m)`. Returns (length, button).
fn sgr_mouse(s: &[u8]) -> Option<(usize, u16)> {
    let body = s.strip_prefix(b"\x1b[<")?;
    let end = body.iter().position(|&b| b == b'M' || b == b'm')?;
    let fields = &body[..end];
    if !fields.iter().all(|b| b.is_ascii_digit() || *b == b';') {
        return None;
    }
    let button = std::str::from_utf8(fields.split(|&b| b == b';').next()?)
        .ok()?
        .parse()
        .ok()?;
    Some((3 + end + 1, button))
}

#[cfg(test)]
mod tests {
    use super::*;
    use Action::*;

    #[test]
    fn plain_keys_forward_untouched() {
        assert_eq!(
            parse(b"ls -la\r", true, 10),
            vec![Forward(b"ls -la\r".to_vec())]
        );
        assert_eq!(
            parse(b"\x1b[A", true, 10),
            vec![Forward(b"\x1b[A".to_vec())]
        );
    }

    #[test]
    fn global_keys() {
        assert_eq!(parse(b"\x11", true, 10), vec![Quit]);
        assert_eq!(parse(b"\x1b`", true, 10), vec![ToggleView]);
        assert_eq!(parse(b"\x1b[5;2~", false, 12), vec![Scroll(12)]);
        assert_eq!(parse(b"\x1b[6;2~", false, 12), vec![Scroll(-12)]);
    }

    #[test]
    fn paste_is_never_interpreted() {
        let p = b"\x1b[200~a\x11b\x1b[201~";
        assert_eq!(parse(p, true, 10), vec![Forward(p.to_vec())]);
    }

    #[test]
    fn wheel_scrolls_and_clicks_are_swallowed_when_we_own_the_mouse() {
        let chunk = b"a\x1b[<64;10;5Mb\x1b[<0;3;4M\x1b[<0;3;4mc\x1b[<65;1;1M";
        assert_eq!(
            parse(chunk, true, 10),
            vec![
                Forward(b"a".to_vec()),
                Scroll(3),
                Forward(b"bc".to_vec()),
                Scroll(-3)
            ]
        );
    }

    #[test]
    fn mouse_goes_to_the_program_when_it_owns_the_mouse() {
        let chunk = b"\x1b[<64;10;5M";
        assert_eq!(parse(chunk, false, 10), vec![Forward(chunk.to_vec())]);
    }
}
