//! Job board: every engine request the UI has in flight, keyed by job id.
//!
//! Views read from this entity instead of owning progress state, so a job keeps
//! reporting correctly while you switch tools.

use std::collections::HashMap;

use serde_json::Value;

use crate::engine::Event;
use crate::success::Success;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    Images,
    Pdf,
    Pages,
    PdfTools,
    Dates,
    Ocr,
    Redact,
    Cleanup,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Images => "Images",
            Tool::Pdf => "Compress PDF",
            Tool::Pages => "PDF pages",
            Tool::PdfTools => "PDF tools",
            Tool::Dates => "Dates",
            Tool::Ocr => "OCR",
            Tool::Redact => "Redact",
            Tool::Cleanup => "Cleanup",
        }
    }
}

#[derive(Debug, Clone)]
pub enum JobState {
    Running,
    Done(Value),
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub tool: Tool,
    pub pct: f32,
    pub note: String,
    pub state: JobState,
    pub log: Vec<String>,
    /// Only published outputs should acknowledge completion; previews stay quiet.
    pub exports_file: bool,
}

impl Job {
    pub fn is_running(&self) -> bool {
        matches!(self.state, JobState::Running)
    }

    pub fn result(&self) -> Option<&Value> {
        match &self.state {
            JobState::Done(value) => Some(value),
            _ => None,
        }
    }

    pub fn error(&self) -> Option<&str> {
        match &self.state {
            JobState::Failed(message) => Some(message),
            _ => None,
        }
    }

    /// Bytes saved, as reported by the engine.
    pub fn saved_label(&self) -> Option<String> {
        let result = self.result()?;
        let saved = result.get("saved").and_then(Value::as_f64)?;
        Some(format!("{:.0}% smaller", saved * 100.0))
    }
}

#[derive(Default)]
pub struct Jobs {
    map: HashMap<String, Job>,
    current: HashMap<Tool, String>,
    saved: HashMap<String, String>,
}

impl gpui_kit::EventEmitter<Success> for Jobs {}

impl Jobs {
    pub fn start(&mut self, id: String, tool: Tool, note: impl Into<String>) {
        self.map.insert(
            id.clone(),
            Job {
                id: id.clone(),
                tool,
                pct: 0.0,
                note: note.into(),
                state: JobState::Running,
                log: Vec::new(),
                exports_file: false,
            },
        );
        self.current.insert(tool, id);
    }

    pub fn start_export(&mut self, id: String, tool: Tool, note: impl Into<String>) {
        self.start(id.clone(), tool, note);
        self.map.get_mut(&id).unwrap().exports_file = true;
    }

    pub fn apply(&mut self, event: Event) -> Option<Success> {
        let mut success = None;
        match event {
            Event::Progress { id, pct, note } => {
                if let Some(job) = self.map.get_mut(&id) {
                    job.pct = pct;
                    if !note.is_empty() {
                        job.note = note;
                    }
                }
            }
            Event::Log { id, message } => {
                if let Some(job) = self.map.get_mut(&id) {
                    job.log.push(message);
                    if job.log.len() > 200 {
                        job.log.remove(0);
                    }
                }
            }
            Event::Done { id, result } => {
                if let Some(job) = self.map.get_mut(&id) {
                    if job.exports_file && job.is_running() {
                        success = Success::exported(&result);
                    }
                    job.pct = 100.0;
                    job.note = "done".into();
                    job.state = JobState::Done(result);
                }
            }
            Event::Error { id, code, message } => {
                if let Some(job) = self.map.get_mut(&id) {
                    job.state = JobState::Failed(if code == "cancelled" {
                        "cancelled".to_string()
                    } else {
                        message
                    });
                    job.note = code;
                }
            }
            Event::Exited(message) => {
                for job in self.map.values_mut() {
                    if job.is_running() {
                        job.state = JobState::Failed(message.clone());
                    }
                }
            }
        }
        success
    }

    pub fn get(&self, id: &Option<String>) -> Option<&Job> {
        id.as_ref().and_then(|id| self.map.get(id))
    }

    pub fn current(&self, tool: Tool) -> Option<&Job> {
        self.current
            .get(&tool)
            .and_then(|id| self.map.get(id))
            .filter(|job| job.is_running())
    }

    pub fn busy(&self, tool: Tool) -> bool {
        self.current(tool).is_some()
    }

    pub fn running_count(&self) -> usize {
        self.map.values().filter(|job| job.is_running()).count()
    }

    pub fn forget_current(&mut self, tool: Tool) {
        if let Some(id) = self.current.remove(&tool) {
            self.map.remove(&id);
        }
    }

    pub fn mark_saved(&mut self, job_id: &str, path: String) {
        self.saved.insert(job_id.to_string(), path);
    }

    pub fn saved_for(&self, job_id: &Option<String>) -> Option<&str> {
        job_id
            .as_ref()
            .and_then(|id| self.saved.get(id))
            .map(String::as_str)
    }
}

#[cfg(test)]
mod success_tests {
    use super::*;
    use serde_json::json;

    fn done(id: &str) -> Event {
        Event::Done {
            id: id.into(),
            result: json!({"dst": "/tmp/output.pdf"}),
        }
    }

    #[test]
    fn only_export_jobs_announce_completion_and_only_once() {
        let mut jobs = Jobs::default();
        jobs.start("preview".into(), Tool::Pdf, "Rendering");
        jobs.start_export("export".into(), Tool::Pdf, "Exporting");
        assert!(jobs.apply(done("preview")).is_none());
        assert!(jobs.apply(done("unknown")).is_none());
        assert!(jobs.apply(done("export")).is_some());
        assert!(jobs.apply(done("export")).is_none());
    }

    #[test]
    fn errors_and_cancellation_never_announce_success() {
        let mut jobs = Jobs::default();
        for code in ["cancelled", "write_failed"] {
            jobs.start_export(code.into(), Tool::Images, "Exporting");
            assert!(
                jobs.apply(Event::Error {
                    id: code.into(),
                    code: code.into(),
                    message: "Could not write".into(),
                })
                .is_none()
            );
            assert!(jobs.apply(done(code)).is_none());
        }
    }
}
