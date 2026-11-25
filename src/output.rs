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
/// asked for it — for a cell with several links, which a TSV cell (one
/// `=HYPERLINK()` formula at most) can't express but a pasted HTML cell can.
#[derive(Default)]
pub struct CopyBuffer {
    pub text: String,
    pub html_rows: String,
    pub needs_html: bool,
}

thread_local! {
    pub static COPY_BUFFER: std::cell::RefCell<Option<CopyBuffer>> = const { std::cell::RefCell::new(None) };
}

/// One output cell: its TSV text, and its HTML for the `--copy` clipboard
/// table.
pub struct Cell {
    pub text: String,
    pub html: String,
}

impl Cell {
    pub fn plain(text: impl Into<String>) -> Cell {
        let text = text.into();
        Cell {
            html: html_escape(&text),
            text,
        }
    }

    pub fn empty() -> Cell {
        Cell::plain("")
    }
}

/// Escapes text for use inside HTML element content or a quoted attribute.
pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Prints a row of cells to stdout as TSV, and — when `--copy` is active —
/// also adds it to the buffer `main` flushes to the clipboard at the end.
/// `clipboard_needs_html` puts the HTML version of the rows on the clipboard
/// too, for cells whose links only survive as HTML.
pub fn emit_cells(cells: &[Cell], clipboard_needs_html: bool) {
    let line = cells
        .iter()
        .map(|cell| cell.text.as_str())
        .collect::<Vec<_>>()
        .join("\t");
    println!("{line}");
    COPY_BUFFER.with(|buffer| {
        if let Some(buffer) = buffer.borrow_mut().as_mut() {
            buffer.text.push_str(&line);
            buffer.text.push('\n');
            buffer.html_rows.push_str("<tr>");
            for cell in cells {
                buffer.html_rows.push_str("<td>");
                buffer.html_rows.push_str(&cell.html);
                buffer.html_rows.push_str("</td>");
            }
            buffer.html_rows.push_str("</tr>");
            buffer.needs_html |= clipboard_needs_html;
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
