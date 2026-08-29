use crate::types::{FrameSemantics, RemoteInputEvent};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub frame_count: u64,
    pub input_count: u64,
    pub total_bytes: u64,
    pub file_path: Option<PathBuf>,
}

pub struct SessionRecorder {
    session: Option<ActiveSession>,
    output_dir: PathBuf,
}

struct ActiveSession {
    id: String,
    started_at: DateTime<Utc>,
    writer: Option<std::io::BufWriter<std::fs::File>>,
    frame_count: u64,
    input_count: u64,
    total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecordedFrame {
    timestamp: DateTime<Utc>,
    semantics: FrameSemantics,
    data_len: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecordedInput {
    timestamp: DateTime<Utc>,
    event: RemoteInputEvent,
}

impl SessionRecorder {
    pub fn new(output_dir: PathBuf) -> Self {
        std::fs::create_dir_all(&output_dir).ok();
        Self {
            session: None,
            output_dir,
        }
    }

    pub fn start_session(&mut self) -> Result<String, std::io::Error> {
        let id = format!("session-{}", Utc::now().format("%Y%m%d-%H%M%S"));
        let file_path = self.output_dir.join(format!("{}.csr", id));
        let file = std::fs::File::create(&file_path)?;
        let writer = std::io::BufWriter::new(file);

        self.session = Some(ActiveSession {
            id: id.clone(),
            started_at: Utc::now(),
            writer: Some(writer),
            frame_count: 0,
            input_count: 0,
            total_bytes: 0,
        });

        tracing::info!(session = %id, path = %file_path.display(), "Recording session started");
        Ok(id)
    }

    pub fn record_frame(&mut self, data: &[u8], semantics: &FrameSemantics) {
        let Some(ref mut session) = self.session else {
            return;
        };
        let Some(ref mut writer) = session.writer else {
            return;
        };

        let record = RecordedFrame {
            timestamp: Utc::now(),
            semantics: semantics.clone(),
            data_len: data.len() as u32,
        };

        let header = serde_json::to_vec(&record).ok();
        if let Some(header) = header {
            let _ = writer.write_all(&(header.len() as u32).to_be_bytes());
            let _ = writer.write_all(&header);
            let _ = writer.write_all(&(data.len() as u32).to_be_bytes());
            let _ = writer.write_all(data);
            let _ = writer.flush();

            session.frame_count += 1;
            session.total_bytes += (header.len() + data.len() + 8) as u64;
        }
    }

    pub fn record_input(&mut self, event: &RemoteInputEvent) {
        let Some(ref mut session) = self.session else {
            return;
        };
        let Some(ref mut writer) = session.writer else {
            return;
        };

        let record = RecordedInput {
            timestamp: Utc::now(),
            event: event.clone(),
        };

        let data = serde_json::to_vec(&record).ok();
        if let Some(data) = data {
            let _ = writer.write_all(&[0xFFu8; 4]); // Marker for input record
            let _ = writer.write_all(&(data.len() as u32).to_be_bytes());
            let _ = writer.write_all(&data);
            let _ = writer.flush();

            session.input_count += 1;
            session.total_bytes += (data.len() + 8) as u64;
        }
    }

    pub fn stop_session(&mut self) -> Option<SessionRecord> {
        let session = self.session.take()?;

        let _ = session.writer.map(|mut w| w.flush());

        let record = SessionRecord {
            session_id: session.id,
            started_at: session.started_at,
            finished_at: Some(Utc::now()),
            frame_count: session.frame_count,
            input_count: session.input_count,
            total_bytes: session.total_bytes,
            file_path: None,
        };

        tracing::info!(
            session = %record.session_id,
            frames = record.frame_count,
            inputs = record.input_count,
            bytes = record.total_bytes,
            "Recording session ended"
        );

        Some(record)
    }

    pub fn is_recording(&self) -> bool {
        self.session.is_some()
    }

    pub fn output_dir(&self) -> &PathBuf {
        &self.output_dir
    }
}

pub use std::io::Write;

impl std::fmt::Display for SessionRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Session {} | {} frames, {} inputs, {:.2} MB",
            self.session_id,
            self.frame_count,
            self.input_count,
            self.total_bytes as f64 / (1024.0 * 1024.0)
        )
    }
}
