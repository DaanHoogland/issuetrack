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

thread_local! {
    pub static COPY_BUFFER: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Prints a data row to stdout, and — when `--copy` is active — also appends
/// it to the in-memory buffer that gets flushed to the clipboard at the end
/// of `main`, so both `--copy` and plain stdout output stay available.
pub fn emit(line: &str) {
    println!("{line}");
    COPY_BUFFER.with(|b| {
        if let Some(buf) = b.borrow_mut().as_mut() {
            buf.push_str(line);
            buf.push('\n');
        }
    });
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
