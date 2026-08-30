use std::io::{self, Write};
use std::path::Path;

use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};

pub(crate) enum Event {
    Offer { x: u16, y: u16 },
    End { canceled: bool },
    Error,
}

pub(crate) fn decode(kind: u8, x: Option<i32>, y: Option<i32>) -> Option<Event> {
    match kind {
        b'o' => Some(Event::Offer {
            x: x?.try_into().ok()?,
            y: y?.try_into().ok()?,
        }),
        b'e' if x == Some(4) => Some(Event::End {
            canceled: y != Some(0),
        }),
        b'E' => Some(Event::Error),
        _ => None,
    }
}

pub(crate) fn enable() -> io::Result<()> {
    supported().then(|| write_osc("t=o:x=1", "")).transpose()?;
    Ok(())
}

pub(crate) fn disable() -> io::Result<()> {
    supported().then(|| write_osc("t=o:x=2", "")).transpose()?;
    Ok(())
}

pub(crate) fn offer(path: &Path, label: &str, icon: Option<(&[u8], u32, u32)>) -> io::Result<()> {
    let uri = file_uri(path);
    let encoded = STANDARD_NO_PAD.encode(uri.as_bytes());
    let mut out = io::stdout().lock();
    write!(
        out,
        "\x1b]72;t=o:o=3;text/uri-list\x1b\\\
         \x1b]72;t=p:x=0:m=0;{encoded}\x1b\\\
         \x1b]72;t=p:x=0\x1b\\"
    )?;
    if let Some((data, width, height)) = icon {
        present_icon(&mut out, 100, width, height, data)?;
    } else {
        present_icon(&mut out, 0, 6, 4, label.as_bytes())?;
    }
    write!(out, "\x1b]72;t=P:x=-1\x1b\\")?;
    out.flush()
}

fn present_icon(
    out: &mut impl Write,
    format: u16,
    width: u32,
    height: u32,
    data: &[u8],
) -> io::Result<()> {
    let encoded = STANDARD_NO_PAD.encode(data);
    let mut chunks = encoded.as_bytes().chunks(4096).peekable();
    if let Some(first) = chunks.next() {
        write!(
            out,
            "\x1b]72;t=p:x=-1:y={format}:X={width}:Y={height}:o=0:m={};{}\x1b\\",
            u8::from(chunks.peek().is_some()),
            String::from_utf8_lossy(first)
        )?;
    }
    while let Some(chunk) = chunks.next() {
        write!(
            out,
            "\x1b]72;m={};{}\x1b\\",
            u8::from(chunks.peek().is_some()),
            String::from_utf8_lossy(chunk)
        )?;
    }
    Ok(())
}

pub(crate) fn reject() -> io::Result<()> {
    write_osc("t=o:o=0", "")
}

fn write_osc(meta: &str, payload: &str) -> io::Result<()> {
    let mut out = io::stdout().lock();
    write!(out, "\x1b]72;{meta};{payload}\x1b\\")?;
    out.flush()
}

fn supported() -> bool {
    if cfg!(windows) {
        return false;
    }
    std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var_os("TERM").is_some_and(|term| term == "xterm-kitty")
}

fn file_uri(path: &Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    #[cfg(windows)]
    let raw = path.to_string_lossy().replace('\\', "/").into_bytes();
    #[cfg(not(windows))]
    let raw = path.as_os_str().as_encoded_bytes().to_vec();
    let mut uri = String::from(if cfg!(windows) { "file:///" } else { "file://" });
    for byte in raw {
        if byte.is_ascii_alphanumeric() || b"-._~/:".contains(&byte) {
            uri.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri.push_str("\r\n");
    uri
}
