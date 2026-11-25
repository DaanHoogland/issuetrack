use anyhow::{Context, Result, bail};
use std::cell::RefCell;
use std::io::Write;
use std::process::{Command, Stdio};

/// How to render a url column: as a plain url, a markdown link, or a Google
/// Sheets `=HYPERLINK()` formula with the given argument separator.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LinkStyle {
    Plain,
    Markdown,
    Hyperlink(char),
}

impl LinkStyle {
    /// `url` alone for `Plain`, else `url` labelled with `text`.
    pub fn format(self, text: &str, url: &str) -> String {
        match self {
            LinkStyle::Plain => url.to_string(),
            LinkStyle::Markdown => format!("[{text}]({url})"),
            LinkStyle::Hyperlink(separator) => {
                format!("=HYPERLINK(\"{url}\"{separator} \"{text}\")")
            }
        }
    }
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

pub fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The rows `--copy` collects while a command runs, as TSV and as an HTML
/// table. The HTML only goes on the clipboard when some row asked for it:
/// a TSV cell holds one `=HYPERLINK()` formula at most, a pasted HTML cell
/// can hold several links.
#[derive(Default)]
struct CopyCapture {
    tsv: String,
    html_rows: String,
    needs_html: bool,
}

thread_local! {
    static COPY_CAPTURE: RefCell<Option<CopyCapture>> = const { RefCell::new(None) };
}

/// Starts collecting emitted rows for `finish_copy_capture`.
pub fn start_copy_capture() {
    COPY_CAPTURE.with(|capture| *capture.borrow_mut() = Some(CopyCapture::default()));
}

/// Puts the rows collected since `start_copy_capture` on the clipboard.
pub fn finish_copy_capture() -> Result<()> {
    let Some(capture) = COPY_CAPTURE.with(|capture| capture.borrow_mut().take()) else {
        return Ok(());
    };
    if capture.tsv.is_empty() {
        return Ok(());
    }
    if capture.needs_html {
        copy_text_and_html_to_clipboard(&capture.tsv, &capture.html_rows)?;
    } else {
        copy_text_to_clipboard(&capture.tsv)?;
    }
    eprintln!(
        "copied {} row(s) to the clipboard",
        capture.tsv.lines().count()
    );
    Ok(())
}

/// Prints a row of cells to stdout as TSV, and adds it to the copy capture
/// when `--copy` is active. `clipboard_needs_html` is for rows with cells
/// whose links only survive as HTML.
pub fn emit_cells(cells: &[Cell], clipboard_needs_html: bool) {
    let tsv_line = cells
        .iter()
        .map(|cell| cell.text.as_str())
        .collect::<Vec<_>>()
        .join("\t");
    print_line_or_exit_when_reader_is_gone(&tsv_line);
    COPY_CAPTURE.with(|capture| {
        if let Some(capture) = capture.borrow_mut().as_mut() {
            capture.tsv.push_str(&tsv_line);
            capture.tsv.push('\n');
            capture.html_rows.push_str(&html_row(cells));
            capture.needs_html |= clipboard_needs_html;
        }
    });
}

/// Exits quietly when stdout's reader has closed (`issuetrack ... | head`),
/// where `println!` would panic.
fn print_line_or_exit_when_reader_is_gone(line: &str) {
    let mut stdout = std::io::stdout().lock();
    if let Err(error) = writeln!(stdout, "{line}") {
        if error.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
        panic!("failed printing to stdout: {error}");
    }
}

fn html_row(cells: &[Cell]) -> String {
    let html_cells: String = cells
        .iter()
        .map(|cell| format!("<td>{}</td>", cell.html))
        .collect();
    format!("<tr>{html_cells}</tr>")
}

fn copy_text_to_clipboard(text: &str) -> Result<()> {
    pipe_into_command("pbcopy", &[], text)
}

/// Reads `{"text": ..., "html": ...}` from stdin and puts both flavors on the
/// macOS pasteboard — `pbcopy` can only write plain text.
const CLIPBOARD_TEXT_AND_HTML_JXA: &str = r#"ObjC.import('AppKit');
function run() {
  const data = $.NSFileHandle.fileHandleWithStandardInput.readDataToEndOfFile;
  const input = JSON.parse($.NSString.alloc.initWithDataEncoding(data, $.NSUTF8StringEncoding).js);
  const pasteboard = $.NSPasteboard.generalPasteboard;
  pasteboard.clearContents;
  pasteboard.setStringForType($(input.html), $.NSPasteboardTypeHTML);
  pasteboard.setStringForType($(input.text), $.NSPasteboardTypeString);
}"#;

/// Apps that understand HTML (Google Sheets) paste the HTML table, keeping
/// every link of a multi-link cell; plain-text targets get the TSV.
fn copy_text_and_html_to_clipboard(tsv: &str, html_rows: &str) -> Result<()> {
    let html_document = format!("<meta charset=\"utf-8\"><table>{html_rows}</table>");
    let payload = serde_json::json!({ "text": tsv, "html": html_document }).to_string();
    pipe_into_command(
        "osascript",
        &["-l", "JavaScript", "-e", CLIPBOARD_TEXT_AND_HTML_JXA],
        &payload,
    )
}

fn pipe_into_command(program: &str, args: &[&str], input: &str) -> Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to run `{program}` — is this macOS?"))?;
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(input.as_bytes())
        .with_context(|| format!("failed to write to {program}"))?;
    let status = child
        .wait()
        .with_context(|| format!("{program} did not exit cleanly"))?;
    if !status.success() {
        bail!("{program} exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_styles() {
        let url = "https://x.y";
        assert_eq!(LinkStyle::Plain.format("t", url), url);
        assert_eq!(LinkStyle::Markdown.format("t", url), "[t](https://x.y)");
        assert_eq!(
            LinkStyle::Hyperlink(';').format("t", url),
            "=HYPERLINK(\"https://x.y\"; \"t\")"
        );
    }

    #[test]
    fn plain_cells_are_html_escaped() {
        let cell = Cell::plain("a <b> & \"c\"");
        assert_eq!(cell.html, "a &lt;b&gt; &amp; &quot;c&quot;");
        assert_eq!(
            html_row(&[cell, Cell::empty()]),
            "<tr><td>a &lt;b&gt; &amp; &quot;c&quot;</td><td></td></tr>"
        );
    }
}
