use anyhow::{Context, Result};

/// How to render a url column: as a plain url, a markdown link, or a Google
/// Sheets `=HYPERLINK()` formula. `--markdown-url` and `--hyperlink-url` are
/// mutually exclusive; neither given means `Plain`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LinkStyle {
    Plain,
    Markdown,
    Hyperlink(char),
}

impl LinkStyle {
    /// Renders `url` alone (`Plain`), or `url` described by `text`
    /// (`Markdown`/`Hyperlink`).
    pub fn format(self, text: &str, url: &str) -> String {
        match self {
            LinkStyle::Plain => url.to_string(),
            LinkStyle::Markdown => format!("[{text}]({url})"),
            LinkStyle::Hyperlink(sep) => format!("=HYPERLINK(\"{url}\"{sep} \"{text}\")"),
        }
    }
}

/// What `--copy` collects while a command runs: the TSV text, and an HTML
/// table of the same rows. The HTML only goes on the clipboard when some row
/// needed it (`rich`) — for a cell with several links, which a TSV cell (one
/// `=HYPERLINK()` formula at most) can't express but a pasted HTML cell can.
#[derive(Default)]
pub struct CopyBuffer {
    pub text: String,
    pub html_rows: String,
    pub rich: bool,
}

thread_local! {
    pub static COPY_BUFFER: std::cell::RefCell<Option<CopyBuffer>> = const { std::cell::RefCell::new(None) };
}

/// Escapes text for use inside HTML element content or a quoted attribute.
pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Prints a data row to stdout, and — when `--copy` is active — also appends
/// it to the in-memory buffer that gets flushed to the clipboard at the end
/// of `main`, so both `--copy` and plain stdout output stay available.
pub fn emit(line: &str) {
    let html_cells = line
        .split('\t')
        .map(|c| format!("<td>{}</td>", html_escape(c)))
        .collect::<String>();
    push(line, &html_cells, false);
}

/// Like `emit`, but with the clipboard's HTML version of the row given
/// explicitly as `<td>` cells, and marking the clipboard as needing HTML.
pub fn emit_rich(line: &str, html_cells: &str) {
    push(line, html_cells, true);
}

fn push(line: &str, html_cells: &str, rich: bool) {
    println!("{line}");
    COPY_BUFFER.with(|b| {
        if let Some(buf) = b.borrow_mut().as_mut() {
            buf.text.push_str(line);
            buf.text.push('\n');
            buf.html_rows.push_str("<tr>");
            buf.html_rows.push_str(html_cells);
            buf.html_rows.push_str("</tr>");
            buf.rich |= rich;
        }
    });
}

/// Puts `text` on the clipboard as plain text and `html` as an HTML table of
/// the same rows, so apps that understand HTML (Google Sheets) keep every
/// link of a multi-link cell, while plain-text targets get the TSV. Uses the
/// macOS pasteboard via JXA, since `pbcopy` only writes plain text.
pub fn copy_rich_to_clipboard(text: &str, html: &str) -> Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    const SCRIPT: &str = r#"ObjC.import('AppKit');
function run() {
  const data = $.NSFileHandle.fileHandleWithStandardInput.readDataToEndOfFile;
  const input = JSON.parse($.NSString.alloc.initWithDataEncoding(data, $.NSUTF8StringEncoding).js);
  const pb = $.NSPasteboard.generalPasteboard;
  pb.clearContents;
  pb.setStringForType($(input.html), $.NSPasteboardTypeHTML);
  pb.setStringForType($(input.text), $.NSPasteboardTypeString);
}"#;
    let html_doc = format!("<meta charset=\"utf-8\"><table>{html}</table>");
    let payload = serde_json::json!({ "text": text, "html": html_doc }).to_string();

    let mut child = Command::new("osascript")
        .args(["-l", "JavaScript", "-e", SCRIPT])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .context("failed to run `osascript` — is this macOS?")?;
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(payload.as_bytes())
        .context("failed to write to osascript")?;
    let status = child.wait().context("osascript did not exit cleanly")?;
    if !status.success() {
        anyhow::bail!("osascript (clipboard) exited with {status}");
    }
    Ok(())
}

/// Pipes `text` to `pbcopy`, replacing the current clipboard contents.
pub fn copy_to_clipboard(text: &str) -> Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .context("failed to run `pbcopy` — is this macOS?")?;
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(text.as_bytes())
        .context("failed to write to pbcopy")?;
    let status = child.wait().context("pbcopy did not exit cleanly")?;
    if !status.success() {
        anyhow::bail!("pbcopy exited with {status}");
    }
    Ok(())
}
