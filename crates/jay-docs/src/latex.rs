//! LaTeX to PDF with Tectonic's engine (MIT; XeTeX in C/C++ inside) linked into Jay-E (decision
//! D6), not a downloaded program. Only the TeX packages a document uses come from the internet, as
//! data, on the first compile (Tectonic's bundle, cached).
//!
//! The engine runs in a child `jay-e.exe` (`ENGINE_ARG`), so a document that crashes TeX or loops
//! never takes the window down: no window, killed after a time limit or when cancelled. Untrusted
//! mode (no shell escape); the PDF goes to a cache folder and the source folder is only read.

use serde::Serialize;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The first argument that makes `jay-e.exe` run the TeX engine instead of the window.
pub const ENGINE_ARG: &str = "--jaye-tex";
/// The longest a compile may take (the first one also downloads the packages it needs).
pub const COMPILE_LIMIT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum LatexError {
    Timeout,
    Cancelled,
    Io { message: String },
}

impl From<std::io::Error> for LatexError {
    fn from(e: std::io::Error) -> Self {
        Self::Io { message: e.to_string() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    /// 1-based line in the .tex, when TeX says.
    pub line: Option<u32>,
    pub message: String,
    pub warning: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Compiled {
    /// The PDF, when TeX produced one (it can, even with errors).
    pub pdf: Option<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
    pub seconds: f64,
}

fn no_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// Compiles `source` (or `text`, the editor's unsaved version of it) into `out_dir`, running the
/// engine in `engine` (Jay-E's own executable) with `ENGINE_ARG`.
pub fn compile(engine: &Path, source: &Path, text: Option<&str>, out_dir: &Path, cancel: &AtomicBool) -> Result<Compiled, LatexError> {
    std::fs::create_dir_all(out_dir)?;
    let name = source.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document.tex".into());
    let stem = Path::new(&name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
    // Unsaved text goes in a copy in the output folder; TeX still sees the source folder's files.
    let input = match text {
        Some(t) => {
            let copy = out_dir.join("unsaved-input.tex");
            std::fs::write(&copy, t)?;
            copy
        }
        None => source.to_path_buf(),
    };
    let pdf = out_dir.join(format!("{stem}.pdf"));
    let _ = std::fs::remove_file(&pdf);
    let root = source.parent().unwrap_or(Path::new("."));
    let started = Instant::now();
    let mut child = no_window(&mut Command::new(engine))
        .arg(ENGINE_ARG)
        .args([input.as_os_str(), root.as_os_str(), out_dir.as_os_str()])
        .arg(&name)
        // Plain "error: ..." lines (termcolor colors them unless TERM says not to).
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    // Read stderr on the side, so a full pipe never blocks the engine.
    let mut stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(e) = stderr.as_mut() {
            let _ = e.take(4 << 20).read_to_string(&mut s);
        }
        s
    });
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if cancel.load(Ordering::Relaxed) || started.elapsed() > COMPILE_LIMIT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if cancel.load(Ordering::Relaxed) { LatexError::Cancelled } else { LatexError::Timeout });
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let stderr = reader.join().unwrap_or_default();
    let log = std::fs::read_to_string(out_dir.join(format!("{stem}.log"))).unwrap_or_default();
    let mut diagnostics = from_stderr(&stderr);
    if !diagnostics.iter().any(|d| !d.warning && d.line.is_some()) {
        diagnostics.extend(from_log(&log));
    }
    Ok(Compiled { pdf: pdf.is_file().then_some(pdf), diagnostics, seconds: started.elapsed().as_secs_f64() })
}

/// The child's side: `[input, root, out_dir, name]` after `ENGINE_ARG`. Compiles the text in
/// `input` as `name`, finding the files it includes (images, .bib) in `root`; messages go to
/// stderr as "error: ..." / "warning: ...". Returns the exit code.
#[cfg(feature = "tex")]
pub fn engine_main(args: &[String]) -> i32 {
    use tectonic::config::PersistentConfig;
    use tectonic::driver::{OutputFormat, ProcessingSessionBuilder};
    use tectonic::status::termcolor::TermcolorStatusBackend;
    use tectonic::status::{ChatterLevel, StatusBackend};
    use tectonic_bridge_core::{SecuritySettings, SecurityStance};

    let mut status = TermcolorStatusBackend::new(ChatterLevel::Minimal);
    status.always_stderr(true);
    let [input, root, out_dir, name] = args else {
        tectonic::tt_error!(status, "expected: input root out_dir name");
        return 2;
    };
    let run = |status: &mut dyn StatusBackend| -> tectonic::Result<()> {
        let text = std::fs::read(input)?;
        let config = PersistentConfig::open(false)?;
        // Untrusted: no shell escape, no search paths outside `root`.
        let mut sb = ProcessingSessionBuilder::new_with_security(SecuritySettings::new(SecurityStance::DisableInsecures));
        sb.bundle(config.default_bundle(false)?)
            .primary_input_buffer(&text)
            .tex_input_name(name)
            .filesystem_root(root)
            .output_dir(out_dir)
            .format_name("latex")
            .format_cache_path(config.format_cache_path()?)
            .keep_logs(true)
            .keep_intermediates(false)
            .print_stdout(false)
            .output_format(OutputFormat::Pdf)
            // \today and the PDF dates: now (or SOURCE_DATE_EPOCH), as the tectonic program does.
            .build_date_from_env(false);
        let mut session = sb.create(status)?;
        session.run(status)
    };
    match run(&mut status) {
        Ok(()) => 0,
        Err(e) => {
            status.report_error(&e);
            1
        }
    }
}

/// Tectonic's own messages: "error: file.tex:12: Undefined control sequence" and "warning: …".
fn from_stderr(stderr: &str) -> Vec<Diagnostic> {
    let mut out = vec![];
    for line in stderr.lines() {
        let (warning, rest) = match (line.strip_prefix("error: "), line.strip_prefix("warning: ")) {
            (Some(r), _) => (false, r),
            (_, Some(r)) => (true, r),
            _ => continue,
        };
        // "<file>:<line>: <message>", the file possibly with a drive's colon in it.
        let parsed = rest.match_indices(':').find_map(|(i, _)| {
            let after = &rest[i + 1..];
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            let tail = after[digits.len()..].strip_prefix(": ")?;
            (!digits.is_empty()).then(|| (digits.parse().ok(), tail.to_owned()))
        });
        let (line_no, message) = parsed.unwrap_or((None, rest.to_owned()));
        if !message.trim().is_empty() {
            out.push(Diagnostic { line: line_no, message: message.trim().to_owned(), warning });
        }
    }
    out
}

/// The TeX log: "! Message." then, a few lines below, "l.12 the source line".
fn from_log(log: &str) -> Vec<Diagnostic> {
    let lines: Vec<&str> = log.lines().collect();
    let mut out = vec![];
    for (i, l) in lines.iter().enumerate() {
        if let Some(message) = l.strip_prefix("! ") {
            let line = lines[i + 1..].iter().take(12).find_map(|n| n.strip_prefix("l.")).and_then(|n| n.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok());
            out.push(Diagnostic { line, message: message.trim().to_owned(), warning: false });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tectonic_messages() {
        let stderr = "note: Running TeX ...\nerror: C:\\Users\\me\\paper.tex:12: Undefined control sequence\nwarning: paper.tex:30: Overfull \\hbox (3.0pt too wide) in paragraph\nerror: halted on potentially-recoverable error as specified\n";
        assert_eq!(
            from_stderr(stderr),
            vec![
                Diagnostic { line: Some(12), message: "Undefined control sequence".into(), warning: false },
                Diagnostic { line: Some(30), message: "Overfull \\hbox (3.0pt too wide) in paragraph".into(), warning: true },
                Diagnostic { line: None, message: "halted on potentially-recoverable error as specified".into(), warning: false },
            ]
        );
    }

    #[test]
    fn tex_log_errors() {
        let log = "(./paper.tex\n! Undefined control sequence.\nl.7 \\foo\n         {bar}\n! Missing $ inserted.\n<inserted text>\n                $\nl.15 a_b\n";
        assert_eq!(
            from_log(log),
            vec![Diagnostic { line: Some(7), message: "Undefined control sequence.".into(), warning: false }, Diagnostic { line: Some(15), message: "Missing $ inserted.".into(), warning: false }]
        );
    }

    #[test]
    fn a_missing_engine_is_an_error_not_a_hang() {
        let out = std::env::temp_dir().join(format!("jay-latex-{}", std::process::id()));
        let r = compile(Path::new("C:/nowhere/jay-e.exe"), Path::new("a.tex"), Some("x"), &out, &AtomicBool::new(false));
        assert!(matches!(r, Err(LatexError::Io { .. })));
        std::fs::remove_dir_all(&out).unwrap();
    }
}
