//! The `.btlog` format of `BT::FileLogger2` (BT.CPP 4.x).
//!
//! The layout comes from `src/loggers/bt_file_logger_v2.cpp` of BT.CPP 4.9.0
//! (constructor `FileLogger2::FileLogger2` and `FileLogger2::writerLoop`):
//!
//! | Bytes | Content |
//! |---|---|
//! | 18 | The text `BTCPP4-FileLogger2`, without a terminator. |
//! | 1 | The format version (`protocol = 1`). |
//! | 4 | The XML length N, `i32` little endian (`flatbuffers::WriteScalar`). |
//! | N | The tree XML from `WriteTreeToXML(tree, true, true)`. |
//! | 8 | The start time, `i64` little endian, microseconds since the Unix epoch. |
//! | 9 each | One record for each status change (see below). |
//!
//! Each record has three fields (`FileLogger2::Transition`, written with
//! `memcpy` on a little-endian host):
//!
//! | Bytes | Content |
//! |---|---|
//! | 6 | The time offset in microseconds since the start time (the low 48 bits of a `u64`). |
//! | 2 | The node UID, `u16` little endian. |
//! | 1 | The new status, 0 to 4 (`BT::NodeStatus`). |
//!
//! `FileLogger2` calls `enableTransitionToIdle(true)`, so the log also has
//! the transitions to IDLE (status 0).
//!
//! The GET_TRANSITIONS reply of the BT.CPP publisher uses
//! the same 9-byte records. [`decode_records`] reads them.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use fasttick_core::runtime::{RuntimeTree, RuntimeTreeError, Uid};
use fasttick_core::status::NodeStatus;
use serde::{Deserialize, Serialize};

/// The text at the start of each `.btlog` file.
pub const MAGIC: &[u8; 18] = b"BTCPP4-FileLogger2";

/// The format version that this crate reads and writes.
pub const FORMAT_VERSION: u8 = 1;

/// The size of one transition record in bytes.
pub const RECORD_SIZE: usize = 9;

/// The largest time offset that a record can hold (48 bits).
pub const MAX_OFFSET_US: u64 = (1 << 48) - 1;

/// Bytes before the XML: magic text, version byte and XML length.
const PREFIX_LEN: usize = MAGIC.len() + 1 + 4;

/// Bytes of the start time after the XML.
const START_TIME_LEN: usize = 8;

/// Errors of this crate.
#[derive(Debug, thiserror::Error)]
pub enum LogError {
    /// A file could not be read.
    #[error("cannot read {path}: {source}")]
    Read {
        /// The file.
        path: PathBuf,
        /// The cause.
        source: io::Error,
    },
    /// A read or write operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// The data is empty.
    #[error("the log is empty")]
    Empty,
    /// The data does not start with `BTCPP4-FileLogger2`.
    #[error("not a BT.CPP FileLogger2 log: the data does not start with \"BTCPP4-FileLogger2\"")]
    WrongMagic,
    /// The version byte is not 1.
    #[error("unsupported .btlog format version {0} (FastTick reads version 1)")]
    UnsupportedVersion(u8),
    /// The XML length in the header is negative.
    #[error("invalid XML length {0} in the header")]
    NegativeXmlLength(i32),
    /// The XML length in the header is larger than the data after it.
    #[error(
        "the XML length in the header ({declared} bytes) is larger than the data after it ({available} bytes)"
    )]
    XmlLengthTooLarge {
        /// The XML length in the header.
        declared: u64,
        /// The number of bytes after the XML length field.
        available: u64,
    },
    /// The data ends inside the header.
    #[error("the log ends inside the header ({available} bytes; the header needs {needed} bytes)")]
    TruncatedHeader {
        /// The number of header bytes that are necessary (as far as known).
        needed: u64,
        /// The number of bytes that are available.
        available: u64,
    },
    /// The tree XML is not valid UTF-8.
    #[error("the tree XML is not valid UTF-8: {0}")]
    XmlNotUtf8(#[source] std::str::Utf8Error),
    /// The tree XML is not valid.
    #[error("the tree XML is not valid: {0}")]
    InvalidXml(#[source] RuntimeTreeError),
    /// The XML is too large for the 32-bit length field.
    #[error("the tree XML is too large for the .btlog format ({0} bytes)")]
    XmlTooLarge(usize),
    /// A time offset does not fit in the 6-byte field of a record.
    #[error("the time offset {0} µs does not fit in the 6-byte field of a record")]
    OffsetTooLarge(u64),
    /// An SQLite operation failed.
    #[cfg(feature = "sqlite")]
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The database does not have a table of the `BT::SqliteLogger` schema.
    #[cfg(feature = "sqlite")]
    #[error("not a BT.CPP SqliteLogger database: the table {0} does not exist")]
    MissingTable(&'static str),
    /// The database has no session with this ID.
    #[cfg(feature = "sqlite")]
    #[error("the database has no session with ID {0}")]
    SessionNotFound(i64),
}

/// One status change of one node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Transition {
    /// Microseconds since the start time of the log.
    pub offset_us: u64,
    /// The UID of the node. It can be a UID that is not in the tree.
    pub uid: Uid,
    /// The new status.
    pub status: NodeStatus,
}

impl Transition {
    /// Makes a transition.
    pub fn new(offset_us: u64, uid: Uid, status: NodeStatus) -> Transition {
        Transition {
            offset_us,
            uid,
            status,
        }
    }
}

/// One record as it is in the data, before the status byte is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RawRecord {
    /// Microseconds since the start time (48 bits).
    pub offset_us: u64,
    /// The node UID.
    pub uid: Uid,
    /// The status byte. BT.CPP writes 0 to 4.
    pub status_byte: u8,
}

impl RawRecord {
    /// Decodes one 9-byte record.
    pub fn decode(bytes: &[u8; RECORD_SIZE]) -> RawRecord {
        let mut time = [0u8; 8];
        time[..6].copy_from_slice(&bytes[..6]);
        RawRecord {
            offset_us: u64::from_le_bytes(time),
            uid: Uid(u16::from_le_bytes([bytes[6], bytes[7]])),
            status_byte: bytes[8],
        }
    }

    /// The transition, or `None` if the status byte is not 0 to 4.
    pub fn transition(&self) -> Option<Transition> {
        NodeStatus::from_u8(self.status_byte).map(|status| Transition {
            offset_us: self.offset_us,
            uid: self.uid,
            status,
        })
    }
}

/// Encodes one transition as a 9-byte record.
///
/// Returns [`LogError::OffsetTooLarge`] if the offset needs more than
/// 48 bits. (BT.CPP keeps only the low 48 bits without a warning.)
pub fn encode_record(transition: &Transition) -> Result<[u8; RECORD_SIZE], LogError> {
    if transition.offset_us > MAX_OFFSET_US {
        return Err(LogError::OffsetTooLarge(transition.offset_us));
    }
    let mut out = [0u8; RECORD_SIZE];
    // bt_file_logger_v2.cpp, FileLogger2::writerLoop: memcpy of 6 bytes of
    // the u64 time offset, 2 bytes of the UID and 1 byte of the status.
    out[..6].copy_from_slice(&transition.offset_us.to_le_bytes()[..6]);
    out[6..8].copy_from_slice(&transition.uid.0.to_le_bytes());
    out[8] = transition.status.as_u8();
    Ok(out)
}

/// The result of [`decode_records`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecodedRecords {
    /// The records with a valid status, in order.
    pub transitions: Vec<Transition>,
    /// The number of records with a status byte that is not 0 to 4.
    pub invalid_records: usize,
    /// The number of bytes at the end that do not make a full record.
    pub trailing_bytes: usize,
}

/// Decodes a sequence of 9-byte records, for example the body of a
/// `.btlog` file or a GET_TRANSITIONS reply.
pub fn decode_records(bytes: &[u8]) -> DecodedRecords {
    let mut out = DecodedRecords {
        transitions: Vec::with_capacity(bytes.len() / RECORD_SIZE),
        ..DecodedRecords::default()
    };
    let chunks = bytes.chunks_exact(RECORD_SIZE);
    out.trailing_bytes = chunks.remainder().len();
    for chunk in chunks {
        let mut record = [0u8; RECORD_SIZE];
        record.copy_from_slice(chunk);
        match RawRecord::decode(&record).transition() {
            Some(transition) => out.transitions.push(transition),
            None => out.invalid_records += 1,
        }
    }
    out
}

/// The header of a `.btlog` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtLogHeader {
    /// The tree XML, as written by `WriteTreeToXML(tree, true, true)`.
    pub xml: String,
    /// The parsed tree.
    pub tree: RuntimeTree,
    /// The start time in microseconds since the Unix epoch.
    pub start_time_us: i64,
    /// The size of the header in bytes. The first record starts here.
    pub len: usize,
}

/// Encodes the header of a `.btlog` file.
pub fn encode_header(xml: &str, start_time_us: i64) -> Result<Vec<u8>, LogError> {
    let xml_len = i32::try_from(xml.len()).map_err(|_| LogError::XmlTooLarge(xml.len()))?;
    let mut out = Vec::with_capacity(PREFIX_LEN + xml.len() + START_TIME_LEN);
    // bt_file_logger_v2.cpp, FileLogger2::FileLogger2: the magic text, the
    // version byte, the XML length (i32), the XML, the start time (i64).
    out.extend_from_slice(MAGIC);
    out.push(FORMAT_VERSION);
    out.extend_from_slice(&xml_len.to_le_bytes());
    out.extend_from_slice(xml.as_bytes());
    out.extend_from_slice(&start_time_us.to_le_bytes());
    Ok(out)
}

/// The progress of the header parser.
enum HeaderProgress {
    /// More bytes are necessary.
    Incomplete {
        /// The total header size if the XML length is known, else the
        /// smallest possible header size.
        needed: usize,
        /// The XML length, if the data has it.
        xml_len: Option<usize>,
    },
    /// The header is complete.
    Complete(BtLogHeader),
}

/// Parses the header at the start of `bytes`.
///
/// Errors are reported as early as possible: a wrong magic text is found
/// at the first wrong byte.
fn parse_header(bytes: &[u8]) -> Result<HeaderProgress, LogError> {
    let magic_len = bytes.len().min(MAGIC.len());
    if bytes[..magic_len] != MAGIC[..magic_len] {
        return Err(LogError::WrongMagic);
    }
    let minimal = PREFIX_LEN + START_TIME_LEN;
    if bytes.len() <= MAGIC.len() {
        return Ok(HeaderProgress::Incomplete {
            needed: minimal,
            xml_len: None,
        });
    }
    let version = bytes[MAGIC.len()];
    if version != FORMAT_VERSION {
        return Err(LogError::UnsupportedVersion(version));
    }
    let Some(len_bytes) = bytes.get(MAGIC.len() + 1..PREFIX_LEN) else {
        return Ok(HeaderProgress::Incomplete {
            needed: minimal,
            xml_len: None,
        });
    };
    let mut raw_len = [0u8; 4];
    raw_len.copy_from_slice(len_bytes);
    let declared = i32::from_le_bytes(raw_len);
    let xml_len = usize::try_from(declared).map_err(|_| LogError::NegativeXmlLength(declared))?;
    let xml_end = PREFIX_LEN.saturating_add(xml_len);
    let total = xml_end.saturating_add(START_TIME_LEN);
    if bytes.len() < total {
        return Ok(HeaderProgress::Incomplete {
            needed: total,
            xml_len: Some(xml_len),
        });
    }
    let xml = std::str::from_utf8(&bytes[PREFIX_LEN..xml_end]).map_err(LogError::XmlNotUtf8)?;
    let mut raw_time = [0u8; 8];
    raw_time.copy_from_slice(&bytes[xml_end..total]);
    let start_time_us = i64::from_le_bytes(raw_time);
    let tree = RuntimeTree::parse(xml).map_err(LogError::InvalidXml)?;
    Ok(HeaderProgress::Complete(BtLogHeader {
        xml: xml.to_owned(),
        tree,
        start_time_us,
        len: total,
    }))
}

/// The error for data that ends inside the header.
fn incomplete_header_error(available: usize, needed: usize, xml_len: Option<usize>) -> LogError {
    if available == 0 {
        return LogError::Empty;
    }
    match xml_len {
        Some(xml_len) if available < PREFIX_LEN + xml_len => LogError::XmlLengthTooLarge {
            declared: xml_len as u64,
            available: available.saturating_sub(PREFIX_LEN) as u64,
        },
        _ => LogError::TruncatedHeader {
            needed: needed as u64,
            available: available as u64,
        },
    }
}

/// A complete `.btlog` log in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtLog {
    /// The tree XML from the header.
    pub xml: String,
    /// The parsed tree.
    pub tree: RuntimeTree,
    /// The start time in microseconds since the Unix epoch.
    pub start_time_us: i64,
    /// The transitions, in the order of the data.
    pub transitions: Vec<Transition>,
    /// The number of bytes at the end that do not make a full record. The
    /// executor can still write the file, so this is not an error.
    pub truncated_bytes: usize,
    /// The number of records with a status byte that is not 0 to 4. The
    /// reader skips these records.
    pub invalid_records: usize,
}

impl BtLog {
    /// Parses a complete `.btlog` file.
    ///
    /// An incomplete last record is not an error: its size is in
    /// [`BtLog::truncated_bytes`].
    pub fn from_bytes(bytes: &[u8]) -> Result<BtLog, LogError> {
        let mut reader = BtLogReader::new();
        reader.push(bytes)?;
        reader.into_log()
    }

    /// Makes a log from its parts, for example from a live recording. The
    /// XML must be valid runtime XML.
    pub fn from_parts(
        xml: String,
        start_time_us: i64,
        transitions: Vec<Transition>,
    ) -> Result<BtLog, LogError> {
        let tree = RuntimeTree::parse(&xml).map_err(LogError::InvalidXml)?;
        Ok(BtLog {
            xml,
            tree,
            start_time_us,
            transitions,
            truncated_bytes: 0,
            invalid_records: 0,
        })
    }

    /// Reads and parses a `.btlog` file.
    pub fn open(path: impl AsRef<Path>) -> Result<BtLog, LogError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|source| LogError::Read {
            path: path.to_owned(),
            source,
        })?;
        BtLog::from_bytes(&bytes)
    }

    /// Writes the log in the `.btlog` format. The incomplete last record
    /// and the invalid records are not written.
    pub fn write_to<W: Write>(&self, writer: W) -> Result<W, LogError> {
        let mut out = BtLogWriter::new(writer, &self.xml, self.start_time_us)?;
        out.push_batch(&self.transitions)?;
        out.flush()?;
        Ok(out.into_inner())
    }

    /// The log in the `.btlog` format. See [`BtLog::write_to`].
    pub fn to_bytes(&self) -> Result<Vec<u8>, LogError> {
        self.write_to(Vec::new())
    }

    /// The largest time offset of all transitions, or 0 if there are none.
    pub fn end_offset_us(&self) -> u64 {
        self.transitions
            .iter()
            .map(|transition| transition.offset_us)
            .max()
            .unwrap_or(0)
    }

    /// The absolute time (microseconds since the Unix epoch) of an offset.
    pub fn absolute_time_us(&self, offset_us: u64) -> i64 {
        absolute_time_us(self.start_time_us, offset_us)
    }

    /// True if the tree has a node with this UID.
    pub fn is_known_uid(&self, uid: Uid) -> bool {
        self.tree.index_of_uid(uid).is_some()
    }

    /// The UIDs that the transitions use but the tree does not have.
    pub fn unknown_uids(&self) -> BTreeSet<Uid> {
        self.transitions
            .iter()
            .map(|transition| transition.uid)
            .filter(|uid| !self.is_known_uid(*uid))
            .collect()
    }
}

/// Adds an offset to a start time. The result saturates at the `i64` limits.
pub fn absolute_time_us(start_time_us: i64, offset_us: u64) -> i64 {
    start_time_us.saturating_add(i64::try_from(offset_us).unwrap_or(i64::MAX))
}

/// An incremental `.btlog` reader.
///
/// Use it for a file that the executor still writes. Give it the bytes as
/// they arrive with [`BtLogReader::push`] or [`BtLogReader::read_from`].
/// The reader keeps an incomplete header or an incomplete last record until
/// more bytes arrive. The result is the same as with [`BtLog::from_bytes`]
/// on all the bytes together.
#[derive(Debug, Clone, Default)]
pub struct BtLogReader {
    /// Bytes that are not used yet: the incomplete header, or after the
    /// header, the bytes of an incomplete record (fewer than 9).
    pending: Vec<u8>,
    /// When the XML length is known: the size of the complete header.
    header_needed: usize,
    header: Option<BtLogHeader>,
    transitions: Vec<Transition>,
    invalid_records: usize,
}

impl BtLogReader {
    /// Makes an empty reader.
    pub fn new() -> BtLogReader {
        BtLogReader::default()
    }

    /// Adds bytes. Returns the number of new valid transitions; they are at
    /// the end of [`BtLogReader::transitions`].
    ///
    /// Header errors are reported as soon as the bytes show them. If the
    /// function returns an error, the reader keeps its state from before
    /// the call.
    pub fn push(&mut self, bytes: &[u8]) -> Result<usize, LogError> {
        let before = self.transitions.len();
        if self.header.is_some() {
            self.decode(bytes);
            return Ok(self.transitions.len() - before);
        }
        if self.pending.is_empty() {
            // No copy for the usual case: all the data in one call.
            match self.try_header(bytes)? {
                Some(header) => {
                    let rest = &bytes[header.len..];
                    self.header = Some(header);
                    self.decode(rest);
                }
                None => self.pending.extend_from_slice(bytes),
            }
        } else {
            let old_len = self.pending.len();
            let mut buffer = std::mem::take(&mut self.pending);
            buffer.extend_from_slice(bytes);
            match self.try_header(&buffer) {
                Ok(Some(header)) => {
                    let start = header.len;
                    self.header = Some(header);
                    self.decode(&buffer[start..]);
                }
                Ok(None) => self.pending = buffer,
                Err(err) => {
                    buffer.truncate(old_len);
                    self.pending = buffer;
                    return Err(err);
                }
            }
        }
        Ok(self.transitions.len() - before)
    }

    /// Reads all bytes that `reader` has now (until it returns 0 bytes) and
    /// adds them. Returns the number of new valid transitions.
    ///
    /// For a file that grows, keep the file open and call this function
    /// again later.
    pub fn read_from<R: Read>(&mut self, reader: &mut R) -> Result<usize, LogError> {
        let mut buffer = vec![0u8; 64 * 1024];
        let mut added = 0;
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => return Ok(added),
                Ok(count) => added += self.push(&buffer[..count])?,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(LogError::Io(err)),
            }
        }
    }

    fn try_header(&mut self, bytes: &[u8]) -> Result<Option<BtLogHeader>, LogError> {
        if bytes.len() < self.header_needed {
            return Ok(None);
        }
        match parse_header(bytes)? {
            HeaderProgress::Complete(header) => Ok(Some(header)),
            HeaderProgress::Incomplete { needed, xml_len } => {
                if xml_len.is_some() {
                    self.header_needed = needed;
                }
                Ok(None)
            }
        }
    }

    fn decode(&mut self, mut bytes: &[u8]) {
        if !self.pending.is_empty() {
            let take = (RECORD_SIZE - self.pending.len()).min(bytes.len());
            self.pending.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.pending.len() < RECORD_SIZE {
                return;
            }
            let mut record = [0u8; RECORD_SIZE];
            record.copy_from_slice(&self.pending);
            self.pending.clear();
            self.add_record(&record);
        }
        let chunks = bytes.chunks_exact(RECORD_SIZE);
        let rest = chunks.remainder();
        self.transitions.reserve(bytes.len() / RECORD_SIZE);
        for chunk in chunks {
            let mut record = [0u8; RECORD_SIZE];
            record.copy_from_slice(chunk);
            self.add_record(&record);
        }
        self.pending.extend_from_slice(rest);
    }

    fn add_record(&mut self, record: &[u8; RECORD_SIZE]) {
        match RawRecord::decode(record).transition() {
            Some(transition) => self.transitions.push(transition),
            None => self.invalid_records += 1,
        }
    }

    /// The header, when it is complete.
    pub fn header(&self) -> Option<&BtLogHeader> {
        self.header.as_ref()
    }

    /// All valid transitions so far, in the order of the data.
    pub fn transitions(&self) -> &[Transition] {
        &self.transitions
    }

    /// The number of records with a status byte that is not 0 to 4.
    pub fn invalid_records(&self) -> usize {
        self.invalid_records
    }

    /// The number of bytes that the reader keeps until more bytes arrive:
    /// an incomplete header, or an incomplete last record.
    pub fn pending_bytes(&self) -> usize {
        self.pending.len()
    }

    /// The log as it is now. See [`BtLogReader::into_log`].
    pub fn to_log(&self) -> Result<BtLog, LogError> {
        self.clone().into_log()
    }

    /// The log as it is now. An incomplete last record is not an error; it
    /// is counted in [`BtLog::truncated_bytes`].
    ///
    /// Returns an error if the header is not complete.
    pub fn into_log(self) -> Result<BtLog, LogError> {
        let Some(header) = self.header else {
            let available = self.pending.len();
            return Err(match parse_header(&self.pending)? {
                HeaderProgress::Incomplete { needed, xml_len } => {
                    incomplete_header_error(available, needed, xml_len)
                }
                // Not possible: `push` takes a complete header at once.
                HeaderProgress::Complete(_) => LogError::TruncatedHeader {
                    needed: available as u64,
                    available: available as u64,
                },
            });
        };
        Ok(BtLog {
            xml: header.xml,
            tree: header.tree,
            start_time_us: header.start_time_us,
            transitions: self.transitions,
            truncated_bytes: self.pending.len(),
            invalid_records: self.invalid_records,
        })
    }
}

/// Writes a `.btlog` file, for example a live recording.
///
/// The output has the same format as `BT::FileLogger2`. For a file, use
/// [`BtLogWriter::create`] or give a buffered writer to
/// [`BtLogWriter::new`].
#[derive(Debug)]
pub struct BtLogWriter<W: Write> {
    inner: W,
    records: u64,
}

impl BtLogWriter<BufWriter<File>> {
    /// Creates (or truncates) a file and writes the header.
    pub fn create(
        path: impl AsRef<Path>,
        xml: &str,
        start_time_us: i64,
    ) -> Result<BtLogWriter<BufWriter<File>>, LogError> {
        let file = File::create(path)?;
        BtLogWriter::new(BufWriter::new(file), xml, start_time_us)
    }
}

impl<W: Write> BtLogWriter<W> {
    /// Writes the header.
    ///
    /// `xml` is the tree XML (for example the FULLTREE reply of the
    /// publisher). The writer does not check it. `start_time_us` is the
    /// start time in microseconds since the Unix epoch. For a live
    /// recording, it is the time in the reply to TOGGLE_RECORDING "start".
    pub fn new(mut inner: W, xml: &str, start_time_us: i64) -> Result<BtLogWriter<W>, LogError> {
        let header = encode_header(xml, start_time_us)?;
        inner.write_all(&header)?;
        Ok(BtLogWriter { inner, records: 0 })
    }

    /// Writes one transition.
    pub fn push(&mut self, transition: Transition) -> Result<(), LogError> {
        let record = encode_record(&transition)?;
        self.inner.write_all(&record)?;
        self.records += 1;
        Ok(())
    }

    /// Writes several transitions. If one offset is too large, the function
    /// writes nothing and returns an error.
    pub fn push_batch(&mut self, transitions: &[Transition]) -> Result<(), LogError> {
        let mut buffer = Vec::with_capacity(transitions.len() * RECORD_SIZE);
        for transition in transitions {
            buffer.extend_from_slice(&encode_record(transition)?);
        }
        self.inner.write_all(&buffer)?;
        self.records += transitions.len() as u64;
        Ok(())
    }

    /// Flushes the inner writer.
    pub fn flush(&mut self) -> Result<(), LogError> {
        self.inner.flush()?;
        Ok(())
    }

    /// The number of records written.
    pub fn records_written(&self) -> u64 {
        self.records
    }

    /// The inner writer.
    pub fn get_ref(&self) -> &W {
        &self.inner
    }

    /// The inner writer. Do not write to it directly, or the file breaks.
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }

    /// Returns the inner writer. It does not flush.
    pub fn into_inner(self) -> W {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<root BTCPP_format="4"><BehaviorTree ID="Main" _fullpath=""><Sequence name="seq" _uid="1"><AlwaysSuccess name="ok" _uid="2"/></Sequence></BehaviorTree></root>"#;

    #[test]
    fn record_layout_matches_btcpp() {
        let transition = Transition::new(0x0605_0403_0201, Uid(0x0A0B), NodeStatus::Failure);
        let bytes = encode_record(&transition).unwrap();
        assert_eq!(bytes, [1, 2, 3, 4, 5, 6, 0x0B, 0x0A, 3]);
        assert_eq!(RawRecord::decode(&bytes).transition(), Some(transition));
    }

    #[test]
    fn largest_offset() {
        let max = Transition::new(MAX_OFFSET_US, Uid(1), NodeStatus::Idle);
        let bytes = encode_record(&max).unwrap();
        assert_eq!(&bytes[..6], &[0xFF; 6]);
        assert_eq!(RawRecord::decode(&bytes).offset_us, MAX_OFFSET_US);
        let too_large = Transition::new(MAX_OFFSET_US + 1, Uid(1), NodeStatus::Idle);
        assert!(matches!(
            encode_record(&too_large),
            Err(LogError::OffsetTooLarge(_))
        ));
    }

    #[test]
    fn header_layout_matches_btcpp() {
        let header = encode_header("<x/>", 0x0102_0304_0506_0708).unwrap();
        let mut expected = b"BTCPP4-FileLogger2".to_vec();
        expected.push(1);
        expected.extend_from_slice(&[4, 0, 0, 0]);
        expected.extend_from_slice(b"<x/>");
        expected.extend_from_slice(&[8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(header, expected);
    }

    #[test]
    fn decode_records_counts_invalid_and_trailing() {
        let mut bytes = encode_record(&Transition::new(5, Uid(1), NodeStatus::Running))
            .unwrap()
            .to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0, 1, 0, 12]);
        bytes.extend_from_slice(&[1, 2, 3]);
        let decoded = decode_records(&bytes);
        assert_eq!(decoded.transitions.len(), 1);
        assert_eq!(decoded.invalid_records, 1);
        assert_eq!(decoded.trailing_bytes, 3);
    }

    #[test]
    fn negative_xml_length() {
        let mut bytes = MAGIC.to_vec();
        bytes.push(1);
        bytes.extend_from_slice(&(-5i32).to_le_bytes());
        assert!(matches!(
            BtLog::from_bytes(&bytes),
            Err(LogError::NegativeXmlLength(-5))
        ));
    }

    #[test]
    fn reader_state_does_not_change_on_error() {
        let header = encode_header(XML, 7).unwrap();
        let mut reader = BtLogReader::new();
        reader.push(&header[..10]).unwrap();
        assert_eq!(reader.pending_bytes(), 10);
        assert!(matches!(reader.push(b"X"), Err(LogError::WrongMagic)));
        assert_eq!(reader.pending_bytes(), 10);
        reader.push(&header[10..]).unwrap();
        assert!(reader.header().is_some());
        assert_eq!(reader.pending_bytes(), 0);
    }

    #[test]
    fn types_can_move_between_threads() {
        fn check<T: Send + Sync + 'static>() {}
        check::<LogError>();
        check::<BtLog>();
        check::<BtLogReader>();
    }

    #[test]
    fn from_parts_checks_the_xml() {
        let log = BtLog::from_parts(XML.to_owned(), 5, vec![]).unwrap();
        assert_eq!(log.tree.len(), 2);
        assert!(matches!(
            BtLog::from_parts("<root/>".to_owned(), 5, vec![]),
            Err(LogError::InvalidXml(_))
        ));
    }

    #[test]
    fn absolute_time_saturates() {
        assert_eq!(absolute_time_us(10, 5), 15);
        assert_eq!(absolute_time_us(i64::MAX - 1, 5), i64::MAX);
        assert_eq!(absolute_time_us(-10, u64::MAX), i64::MAX - 10);
    }

    #[test]
    fn unknown_uids_are_kept() {
        let mut writer = BtLogWriter::new(Vec::new(), XML, 0).unwrap();
        writer
            .push(Transition::new(1, Uid(99), NodeStatus::Running))
            .unwrap();
        let log = BtLog::from_bytes(writer.get_ref()).unwrap();
        assert_eq!(log.transitions.len(), 1);
        assert_eq!(
            log.unknown_uids().into_iter().collect::<Vec<_>>(),
            [Uid(99)]
        );
        assert!(log.is_known_uid(Uid(1)));
    }
}
