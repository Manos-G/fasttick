//! A live recording: the transitions that the publisher records, and their
//! conversion to a `.btlog` file.
//!
//! The publisher gives the start time of a recording in the reply to
//! TOGGLE_RECORDING "start" (microseconds since the Unix epoch), and each
//! recorded transition has a time offset from that start
//! (`serverLoop` of the publisher, cases `TOGGLE_RECORDING` and
//! `GET_TRANSITIONS`). A `.btlog` file has the same parts: a start time
//! and transitions with an offset (`BT::FileLogger2`). The conversion is
//! therefore one to one.

use std::path::Path;
use std::time::{Duration, Instant};

use fasttick_log::btlog::{BtLogWriter, LogError, Transition};
use fasttick_protocol::messages::RecordedTransition;

/// The transitions of a recording, while it runs and after it stopped.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recording {
    active: bool,
    start_time_us: Option<u64>,
    xml: Option<String>,
    transitions: Vec<RecordedTransition>,
    lossy_batches: u32,
    started_at: Option<Instant>,
    stopped_at: Option<Instant>,
}

/// The content of a `.btlog` file made from a recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingFile {
    /// The tree XML (the FULLTREE reply of the executor).
    pub xml: String,
    /// The start time in microseconds since the Unix epoch.
    pub start_time_us: i64,
    /// The transitions.
    pub transitions: Vec<Transition>,
}

impl Recording {
    /// Starts a new recording. The old transitions are discarded.
    pub fn start(&mut self, start_time_us: u64, xml: Option<String>, now: Instant) {
        *self = Recording {
            active: true,
            start_time_us: Some(start_time_us),
            xml,
            started_at: Some(now),
            ..Recording::default()
        };
    }

    /// Adds a batch of transitions. `possible_loss` is true when the
    /// publisher buffer was full: older transitions can be lost.
    pub fn push(&mut self, batch: &[RecordedTransition], possible_loss: bool) {
        if self.start_time_us.is_none() {
            return;
        }
        self.transitions.extend_from_slice(batch);
        if possible_loss {
            self.lossy_batches += 1;
        }
    }

    /// Marks the recording as stopped.
    pub fn stop(&mut self, now: Instant) {
        if self.active {
            self.active = false;
            self.stopped_at = Some(now);
        }
    }

    /// True while the recording runs.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// True if a recording started (it can run or be stopped).
    pub fn has_data(&self) -> bool {
        self.start_time_us.is_some()
    }

    /// True if no transition was recorded.
    pub fn is_empty(&self) -> bool {
        self.transitions.is_empty()
    }

    /// The number of transitions.
    pub fn len(&self) -> usize {
        self.transitions.len()
    }

    /// The transitions, oldest first.
    pub fn transitions(&self) -> &[RecordedTransition] {
        &self.transitions
    }

    /// True if transitions can be lost (a full buffer).
    pub fn possible_loss(&self) -> bool {
        self.lossy_batches > 0
    }

    /// The number of batches that filled the buffer of the publisher.
    pub fn lossy_batches(&self) -> u32 {
        self.lossy_batches
    }

    /// The tree XML at the start of the recording.
    pub fn xml(&self) -> Option<&str> {
        self.xml.as_deref()
    }

    /// The start time from the executor, in microseconds since the Unix
    /// epoch.
    pub fn start_time_us(&self) -> Option<u64> {
        self.start_time_us
    }

    /// How long the recording ran (until now while it runs).
    pub fn duration(&self, now: Instant) -> Duration {
        match (self.started_at, self.stopped_at) {
            (Some(start), Some(stop)) => stop.saturating_duration_since(start),
            (Some(start), None) => now.saturating_duration_since(start),
            _ => Duration::ZERO,
        }
    }

    /// The file content. `fallback_xml` is used if the tree was not known
    /// at the start. `None` if no recording started or no XML is known.
    pub fn to_file(&self, fallback_xml: Option<&str>) -> Option<RecordingFile> {
        let start = self.start_time_us?;
        let xml = self.xml.as_deref().or(fallback_xml)?.to_owned();
        Some(RecordingFile {
            xml,
            start_time_us: start_time(start),
            transitions: to_log_transitions(&self.transitions),
        })
    }
}

/// Converts the start time of the publisher to the `.btlog` start time.
/// The value saturates at `i64::MAX`.
pub fn start_time(start_time_us: u64) -> i64 {
    i64::try_from(start_time_us).unwrap_or(i64::MAX)
}

/// Converts recorded transitions to log transitions. The offsets stay the
/// same: both count from the start of the recording.
pub fn to_log_transitions(recorded: &[RecordedTransition]) -> Vec<Transition> {
    recorded
        .iter()
        .map(|t| Transition::new(t.timestamp_us, t.uid, t.status))
        .collect()
}

/// Writes a recording as a `.btlog` file. Returns the number of records.
pub fn write_btlog(path: &Path, file: &RecordingFile) -> Result<u64, LogError> {
    let mut writer = BtLogWriter::create(path, &file.xml, file.start_time_us)?;
    writer.push_batch(&file.transitions)?;
    writer.flush()?;
    Ok(writer.records_written())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fasttick_core::runtime::Uid;
    use fasttick_core::status::NodeStatus;
    use fasttick_log::btlog::BtLog;

    const XML: &str = r#"<root BTCPP_format="4"><BehaviorTree ID="T" _fullpath=""><Sequence _uid="1"><AlwaysSuccess _uid="2"/></Sequence></BehaviorTree></root>"#;

    fn rec(timestamp_us: u64, uid: u16, status: NodeStatus) -> RecordedTransition {
        RecordedTransition {
            timestamp_us,
            uid: Uid(uid),
            status,
        }
    }

    #[test]
    fn records_and_writes_a_btlog_file() {
        let now = Instant::now();
        let mut recording = Recording::default();
        assert!(!recording.has_data());
        // A batch before the start is ignored.
        recording.push(&[rec(1, 1, NodeStatus::Running)], false);
        assert!(recording.is_empty());
        assert!(recording.to_file(Some(XML)).is_none());

        recording.start(1_700_000_000_000_000, Some(XML.to_owned()), now);
        assert!(recording.is_active());
        recording.push(
            &[
                rec(10, 1, NodeStatus::Running),
                rec(15, 2, NodeStatus::Success),
            ],
            false,
        );
        recording.push(&[rec(20, 1, NodeStatus::Success)], true);
        assert!(recording.possible_loss());
        assert_eq!(recording.lossy_batches(), 1);
        assert_eq!(recording.len(), 3);
        recording.stop(now + Duration::from_secs(2));
        assert!(!recording.is_active());
        assert_eq!(
            recording.duration(now + Duration::from_secs(9)).as_secs(),
            2
        );

        let file = recording.to_file(None).unwrap();
        assert_eq!(file.start_time_us, 1_700_000_000_000_000);
        assert_eq!(
            file.transitions[1],
            Transition::new(15, Uid(2), NodeStatus::Success)
        );

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rec.btlog");
        assert_eq!(write_btlog(&path, &file).unwrap(), 3);
        let log = BtLog::open(&path).unwrap();
        assert_eq!(log.xml, XML);
        assert_eq!(log.start_time_us, file.start_time_us);
        assert_eq!(log.transitions, file.transitions);

        // A bad path gives an error, not a panic.
        assert!(write_btlog(&dir.path().join("no/such/dir.btlog"), &file).is_err());
    }

    #[test]
    fn fallback_xml_and_start_time() {
        let mut recording = Recording::default();
        recording.start(5, None, Instant::now());
        assert!(recording.to_file(None).is_none());
        assert_eq!(recording.to_file(Some(XML)).unwrap().xml, XML);
        assert_eq!(start_time(u64::MAX), i64::MAX);
        assert_eq!(start_time(7), 7);
    }
}
