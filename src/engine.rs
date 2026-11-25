//! Supervisor for the Python engine sidecar.
//!
//! One long-lived child process, newline-delimited JSON over stdin/stdout, and
//! a reader thread that turns replies into events. No sockets are opened.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::Value;

#[derive(Debug, Clone)]
pub enum Event {
    Progress {
        id: String,
        pct: f32,
        note: String,
    },
    Log {
        id: String,
        message: String,
    },
    Done {
        id: String,
        result: Value,
    },
    Error {
        id: String,
        code: String,
        message: String,
    },
    /// The sidecar went away; every running job is dead.
    Exited(String),
}

#[derive(Debug, Clone, Default)]
pub struct Features {
    pub pdf: bool,
    pub ocr: bool,
    pub ghostscript: bool,
    pub heif: bool,
    pub avif: bool,
    pub c2pa: bool,
    pub version: String,
    pub languages: Vec<String>,
}

impl Features {
    fn from_result(value: &Value) -> Self {
        let f = value.get("features").cloned().unwrap_or(Value::Null);
        let flag = |key: &str| f.get(key).and_then(Value::as_bool).unwrap_or(false);
        Features {
            pdf: flag("pdf"),
            ocr: flag("ocr"),
            ghostscript: flag("ghostscript"),
            heif: flag("heif"),
            avif: flag("avif"),
            c2pa: flag("c2pa"),
            version: value
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string(),
            languages: value
                .get("languages")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

struct Inner {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    counter: AtomicU64,
    events: async_channel::Receiver<Event>,
    features: Arc<Mutex<Features>>,
}

/// Cheap handle to the engine; clone it into every view that needs work done.
#[derive(Clone)]
pub struct Engine(Arc<Inner>);

impl Engine {
    pub fn spawn() -> Result<Self, String> {
        let mut command = resolve_command();
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW for the sidecar.
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("PYTHONUNBUFFERED", "1");

        let mut child = command
            .spawn()
            .map_err(|err| format!("could not start the engine: {err}"))?;
        let stdin = child.stdin.take().ok_or("engine stdin missing")?;
        let stdout = child.stdout.take().ok_or("engine stdout missing")?;
        let stderr = child.stderr.take().ok_or("engine stderr missing")?;

        let (tx, rx) = async_channel::unbounded();
        let features = Arc::new(Mutex::new(Features::default()));
        let read_features = features.clone();

        thread::Builder::new()
            .name("garage-engine-out".into())
            .spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let Some(event) = parse_event(&line) else {
                        continue;
                    };
                    // The first ping populates the capability badge in the UI.
                    if let Event::Done { id, result } = &event
                        && id.starts_with("ping")
                        && let Ok(mut slot) = read_features.lock()
                    {
                        *slot = Features::from_result(result);
                    }
                    if tx.send_blocking(event).is_err() {
                        break;
                    }
                }
                let _ = tx.send_blocking(Event::Exited(
                    "engine process ended unexpectedly".to_string(),
                ));
            })
            .map_err(|err| err.to_string())?;

        thread::Builder::new()
            .name("garage-engine-err".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    eprintln!("[engine] {line}");
                }
            })
            .map_err(|err| err.to_string())?;

        let engine = Engine(Arc::new(Inner {
            stdin: Mutex::new(stdin),
            child: Mutex::new(child),
            counter: AtomicU64::new(1),
            events: rx,
            features,
        }));
        engine.ping();
        Ok(engine)
    }

    pub fn events(&self) -> async_channel::Receiver<Event> {
        self.0.events.clone()
    }

    pub fn features(&self) -> Features {
        self.0
            .features
            .lock()
            .map(|f| f.clone())
            .unwrap_or_default()
    }

    /// Queue an operation; returns the job id used by all subsequent events.
    pub fn call(&self, op: &str, args: Value) -> String {
        let id = format!("j{}", self.0.counter.fetch_add(1, Ordering::SeqCst));
        self.send(&serde_json::json!({ "id": id, "op": op, "args": args }));
        id
    }

    pub fn cancel(&self, id: &str) {
        self.send(&serde_json::json!({ "id": id, "op": "cancel" }));
    }

    fn send(&self, message: &Value) {
        if let Ok(mut stdin) = self.0.stdin.lock() {
            let _ = writeln!(stdin, "{message}");
            let _ = stdin.flush();
        }
    }

    fn ping(&self) {
        self.send(&serde_json::json!({ "id": "ping-1", "op": "ping" }));
    }

    pub fn kill(&self) {
        if let Ok(mut child) = self.0.child.lock() {
            let _ = child.kill();
        }
    }
}

fn parse_event(line: &str) -> Option<Event> {
    let value: Value = serde_json::from_str(line).ok()?;
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    match value.get("event").and_then(Value::as_str)? {
        "progress" => Some(Event::Progress {
            id,
            pct: value.get("pct").and_then(Value::as_f64).unwrap_or(0.0) as f32,
            note: value
                .get("note")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }),
        "log" => Some(Event::Log {
            id,
            message: value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }),
        "done" => Some(Event::Done {
            id,
            result: value.get("result").cloned().unwrap_or(Value::Null),
        }),
        "error" => Some(Event::Error {
            id,
            code: value
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("internal")
                .to_string(),
            message: value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_string(),
        }),
        _ => None,
    }
}

/// Release build: a sibling `garage-engine` binary.
/// Development: the project venv, so Python edits apply instantly.
fn resolve_command() -> Command {
    if let Ok(path) = std::env::var("GARAGE_ENGINE") {
        return Command::new(path);
    }
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.with_file_name(if cfg!(target_os = "windows") {
            "garage-engine.exe"
        } else {
            "garage-engine"
        });
        if sibling.is_file() {
            return Command::new(sibling);
        }
    }
    let root = project_root();
    let venv = root.join(if cfg!(target_os = "windows") {
        ".venv/Scripts/python.exe"
    } else {
        ".venv/bin/python"
    });
    if venv.is_file() {
        let mut command = Command::new(venv);
        command
            .arg(root.join("engine/run.py"))
            .env("PYTHONPATH", root.join("engine"));
        return command;
    }
    let mut command = Command::new("uv");
    command
        .args(["run", "--project", "engine", "python", "engine/run.py"])
        .current_dir(&root);
    command
}

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
