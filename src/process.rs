use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{self, Sender};
use std::thread;

use crate::error::{Result, ZemError};

const TAIL_LINES: usize = 25;

/// Runs external tools, mirroring their output into a log file and keeping the last lines
/// for error reports. Output is echoed to the terminal only in verbose mode.
pub struct Runner {
    log: Option<Mutex<File>>,
    log_path: Option<PathBuf>,
    verbose: bool,
}

impl Runner {
    pub fn new(log_path: Option<PathBuf>, verbose: bool) -> Result<Self> {
        let log = match &log_path {
            Some(path) => {
                if let Some(parent) = path.parent() {
                    crate::fsutil::create_dir_all(parent)?;
                }
                Some(Mutex::new(File::create(path).map_err(ZemError::io(path))?))
            }
            None => None,
        };
        Ok(Runner {
            log,
            log_path,
            verbose,
        })
    }

    pub fn verbose(&self) -> bool {
        self.verbose
    }

    pub fn log_path(&self) -> Option<&Path> {
        self.log_path.as_deref()
    }

    pub fn note(&self, line: &str) {
        if let Some(log) = &self.log
            && let Ok(mut file) = log.lock()
        {
            let _ = writeln!(file, "{line}");
        }
    }

    fn failure_advice(&self) -> Option<String> {
        let log = self
            .log_path
            .as_ref()
            .map(|p| format!("full log: {}", p.display()));
        let rerun =
            (!self.verbose).then(|| "rerun with --verbose to stream the output".to_string());
        let parts: Vec<String> = log.into_iter().chain(rerun).collect();
        (!parts.is_empty()).then(|| parts.join("; "))
    }

    pub fn run(&self, cmd: &mut Command) -> Result<()> {
        self.execute(cmd).map(|_| ())
    }

    /// Like [`Runner::run`] but also returns stdout.
    pub fn output(&self, cmd: &mut Command) -> Result<String> {
        self.execute(cmd)
    }

    fn execute(&self, cmd: &mut Command) -> Result<String> {
        let display = display_command(cmd);
        self.note(&format!("$ {display}"));
        if self.verbose {
            eprintln!("    $ {display}");
        }

        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|source| ZemError::Spawn {
            command: display.clone(),
            source,
        })?;

        let (tx, rx) = mpsc::channel();
        let readers = [
            spawn_reader(child.stdout.take(), Stream::Stdout, tx.clone()),
            spawn_reader(child.stderr.take(), Stream::Stderr, tx),
        ];

        let mut stdout = String::new();
        let mut tail = VecDeque::with_capacity(TAIL_LINES);
        for (stream, line) in rx {
            self.note(&line);
            if self.verbose {
                eprintln!("    {line}");
            }
            if stream == Stream::Stdout {
                stdout.push_str(&line);
                stdout.push('\n');
            }
            if tail.len() == TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line);
        }
        for reader in readers.into_iter().flatten() {
            let _ = reader.join();
        }

        let status = child.wait().map_err(|source| ZemError::Spawn {
            command: display.clone(),
            source,
        })?;
        if status.success() {
            Ok(stdout)
        } else {
            Err(ZemError::CommandFailed {
                command: display,
                status: status.to_string(),
                tail: Vec::from(tail).join("\n"),
                advice: self.failure_advice(),
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    Stdout,
    Stderr,
}

fn spawn_reader<R: Read + Send + 'static>(
    pipe: Option<R>,
    stream: Stream,
    tx: Sender<(Stream, String)>,
) -> Option<thread::JoinHandle<()>> {
    let pipe = pipe?;
    Some(thread::spawn(move || {
        for chunk in BufReader::new(pipe).split(b'\n') {
            let Ok(chunk) = chunk else { break };
            let text = String::from_utf8_lossy(&chunk);
            // Progress bars redraw with '\r'; keep only the final state of the line.
            let line = text
                .rsplit('\r')
                .find(|s| !s.trim().is_empty())
                .unwrap_or("");
            if tx.send((stream, line.trim_end().to_string())).is_err() {
                break;
            }
        }
    }))
}

pub fn display_command(cmd: &Command) -> String {
    std::iter::once(cmd.get_program())
        .chain(cmd.get_args())
        .map(|s| {
            let s = s.to_string_lossy();
            if s.contains(char::is_whitespace) {
                format!("{s:?}")
            } else {
                s.into_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn which(name: &str) -> Option<PathBuf> {
        which::which(name).ok()
    }

    #[test]
    fn display_quotes_arguments_with_spaces() {
        let mut cmd = Command::new("west");
        cmd.args(["build", "-d", "my build"]);
        assert_eq!(display_command(&cmd), r#"west build -d "my build""#);
    }

    #[test]
    fn captures_stdout_and_logs() {
        let Some(git) = which("git") else { return };
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("log.txt");
        let runner = Runner::new(Some(log.clone()), false).unwrap();
        let out = runner.output(Command::new(git).arg("--version")).unwrap();
        assert!(out.starts_with("git version"));
        let logged = std::fs::read_to_string(log).unwrap();
        assert!(logged.contains("--version") && logged.contains("git version"));
    }

    #[test]
    fn failure_reports_tail() {
        let Some(git) = which("git") else { return };
        let runner = Runner::new(None, false).unwrap();
        let err = runner
            .run(Command::new(git).arg("definitely-not-a-git-command"))
            .unwrap_err();
        match err {
            ZemError::CommandFailed { tail, .. } => {
                assert!(tail.contains("definitely-not-a-git-command"))
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
