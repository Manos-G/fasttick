//! Tests of the `.btlog` reader and writer.

mod support;

use std::fs::OpenOptions;
use std::io::Write;

use fasttick_core::runtime::Uid;
use fasttick_core::status::NodeStatus;
use fasttick_log::btlog::{
    BtLog, BtLogReader, BtLogWriter, LogError, MAX_OFFSET_US, RECORD_SIZE, Transition,
    encode_header,
};
use proptest::prelude::*;
use support::{SAMPLE_XML, START_US, handmade_log, t, transitions_strategy};

fn sample_transitions() -> Vec<Transition> {
    vec![
        t(120, 1, NodeStatus::Running),
        t(130, 2, NodeStatus::Running),
        t(5_000, 2, NodeStatus::Success),
        t(5_010, 3, NodeStatus::Failure),
        t(5_011, 1, NodeStatus::Failure),
        t(5_020, 2, NodeStatus::Idle),
        t(5_020, 3, NodeStatus::Idle),
        t(5_021, 1, NodeStatus::Idle),
        t(MAX_OFFSET_US, 8, NodeStatus::Skipped),
    ]
}

fn write_log(xml: &str, start: i64, transitions: &[Transition]) -> Vec<u8> {
    let mut writer = BtLogWriter::new(Vec::new(), xml, start).unwrap();
    writer.push_batch(transitions).unwrap();
    writer.flush().unwrap();
    writer.into_inner()
}

fn header_len(xml: &str) -> usize {
    18 + 1 + 4 + xml.len() + 8
}

#[test]
fn writer_matches_handmade_bytes() {
    let transitions = sample_transitions();
    let records: Vec<(u64, u16, u8)> = transitions
        .iter()
        .map(|tr| (tr.offset_us, tr.uid.0, tr.status.as_u8()))
        .collect();
    let expected = handmade_log(SAMPLE_XML, START_US, &records);
    assert_eq!(write_log(SAMPLE_XML, START_US, &transitions), expected);

    // `push` one by one gives the same bytes as `push_batch`.
    let mut writer = BtLogWriter::new(Vec::new(), SAMPLE_XML, START_US).unwrap();
    for transition in &transitions {
        writer.push(*transition).unwrap();
    }
    assert_eq!(writer.records_written(), transitions.len() as u64);
    assert_eq!(writer.into_inner(), expected);
}

#[test]
fn round_trip() {
    let transitions = sample_transitions();
    let bytes = write_log(SAMPLE_XML, START_US, &transitions);
    let log = BtLog::from_bytes(&bytes).unwrap();
    assert_eq!(log.xml, SAMPLE_XML);
    assert_eq!(log.start_time_us, START_US);
    assert_eq!(log.transitions, transitions);
    assert_eq!(log.truncated_bytes, 0);
    assert_eq!(log.invalid_records, 0);
    assert_eq!(log.tree.len(), 8);
    assert_eq!(
        log.tree.node_by_uid(Uid(6)).unwrap().type_id,
        "ApproachDock"
    );
    assert_eq!(log.end_offset_us(), MAX_OFFSET_US);
    assert_eq!(log.absolute_time_us(120), START_US + 120);
    assert_eq!(log.to_bytes().unwrap(), bytes);
}

#[test]
fn empty_transition_list() {
    let bytes = write_log(SAMPLE_XML, START_US, &[]);
    assert_eq!(bytes.len(), header_len(SAMPLE_XML));
    let log = BtLog::from_bytes(&bytes).unwrap();
    assert!(log.transitions.is_empty());
    assert_eq!(log.truncated_bytes, 0);
    assert_eq!(log.end_offset_us(), 0);
    assert_eq!(log.to_bytes().unwrap(), bytes);
}

#[test]
fn negative_start_time() {
    let bytes = write_log(SAMPLE_XML, -42, &[t(1, 1, NodeStatus::Running)]);
    let log = BtLog::from_bytes(&bytes).unwrap();
    assert_eq!(log.start_time_us, -42);
    assert_eq!(log.absolute_time_us(1), -41);
}

#[test]
fn truncated_header() {
    let bytes = write_log(SAMPLE_XML, START_US, &[]);
    assert!(matches!(BtLog::from_bytes(&[]), Err(LogError::Empty)));
    for cut in 1..bytes.len() {
        let result = BtLog::from_bytes(&bytes[..cut]);
        let xml_end = 23 + SAMPLE_XML.len();
        match result {
            Err(LogError::TruncatedHeader { needed, available }) => {
                assert!(cut < 23 || cut >= xml_end, "cut {cut}");
                assert_eq!(available, cut as u64);
                assert!(needed > available);
                if cut >= 23 {
                    assert_eq!(needed, bytes.len() as u64);
                }
            }
            Err(LogError::XmlLengthTooLarge {
                declared,
                available,
            }) => {
                assert!((23..xml_end).contains(&cut), "cut {cut}");
                assert_eq!(declared, SAMPLE_XML.len() as u64);
                assert_eq!(available, (cut - 23) as u64);
            }
            other => panic!("cut {cut}: unexpected result {other:?}"),
        }
    }
}

#[test]
fn truncated_last_record_is_kept_back() {
    let transitions = sample_transitions();
    let bytes = write_log(SAMPLE_XML, START_US, &transitions);
    for missing in 1..RECORD_SIZE {
        let cut = &bytes[..bytes.len() - missing];
        let log = BtLog::from_bytes(cut).unwrap();
        assert_eq!(log.transitions, transitions[..transitions.len() - 1]);
        assert_eq!(log.truncated_bytes, RECORD_SIZE - missing);
        assert_eq!(log.invalid_records, 0);
    }

    // The incremental reader completes the record when the rest arrives.
    let mut reader = BtLogReader::new();
    let split = bytes.len() - 4;
    assert_eq!(reader.push(&bytes[..split]).unwrap(), transitions.len() - 1);
    assert_eq!(reader.pending_bytes(), RECORD_SIZE - 4);
    assert_eq!(reader.to_log().unwrap().truncated_bytes, RECORD_SIZE - 4);
    assert_eq!(reader.push(&bytes[split..]).unwrap(), 1);
    assert_eq!(reader.pending_bytes(), 0);
    assert_eq!(reader.transitions(), transitions);
}

#[test]
fn byte_by_byte_equals_all_at_once() {
    let mut bytes = write_log(SAMPLE_XML, START_US, &sample_transitions());
    // An invalid record and an incomplete record at the end.
    bytes.extend_from_slice(&[1, 0, 0, 0, 0, 0, 2, 0, 9]);
    bytes.extend_from_slice(&[7, 7, 7]);
    let all = BtLog::from_bytes(&bytes).unwrap();
    assert_eq!(all.invalid_records, 1);
    assert_eq!(all.truncated_bytes, 3);

    let mut reader = BtLogReader::new();
    let mut new_total = 0;
    for (position, byte) in bytes.iter().enumerate() {
        new_total += reader.push(std::slice::from_ref(byte)).unwrap();
        let header_done = position + 1 >= header_len(SAMPLE_XML);
        assert_eq!(reader.header().is_some(), header_done, "byte {position}");
        if !header_done {
            assert_eq!(reader.pending_bytes(), position + 1);
            assert!(reader.to_log().is_err());
        }
    }
    assert_eq!(new_total, all.transitions.len());
    assert_eq!(reader.into_log().unwrap(), all);
}

#[test]
fn wrong_magic() {
    let good = write_log(SAMPLE_XML, START_US, &[]);
    for position in 0..18 {
        let mut bytes = good.clone();
        bytes[position] ^= 0x20;
        assert!(
            matches!(BtLog::from_bytes(&bytes), Err(LogError::WrongMagic)),
            "position {position}"
        );
        // The incremental reader reports it at the first wrong byte.
        let mut reader = BtLogReader::new();
        reader.push(&bytes[..position]).unwrap();
        assert!(matches!(
            reader.push(&bytes[position..=position]),
            Err(LogError::WrongMagic)
        ));
    }
    assert!(matches!(
        BtLog::from_bytes(b"BTCPP3-FileLogger"),
        Err(LogError::WrongMagic)
    ));
    assert!(matches!(
        BtLog::from_bytes(b"<root/>"),
        Err(LogError::WrongMagic)
    ));
}

#[test]
fn unsupported_versions() {
    let good = write_log(SAMPLE_XML, START_US, &[]);
    for version in [0u8, 2, 255] {
        let mut bytes = good.clone();
        bytes[18] = version;
        match BtLog::from_bytes(&bytes) {
            Err(LogError::UnsupportedVersion(found)) => assert_eq!(found, version),
            other => panic!("unexpected result {other:?}"),
        }
        // Found as soon as the version byte arrives.
        let mut reader = BtLogReader::new();
        assert!(matches!(
            reader.push(&bytes[..19]),
            Err(LogError::UnsupportedVersion(_))
        ));
    }
}

#[test]
fn unknown_status_bytes_are_counted_and_skipped() {
    let records = [
        (10, 1, 1),
        (11, 1, 5),
        (12, 2, 11),
        (13, 2, 2),
        (14, 3, 255),
        (15, 1, 0),
    ];
    let bytes = handmade_log(SAMPLE_XML, START_US, &records);
    let log = BtLog::from_bytes(&bytes).unwrap();
    assert_eq!(log.invalid_records, 3);
    assert_eq!(
        log.transitions,
        vec![
            t(10, 1, NodeStatus::Running),
            t(13, 2, NodeStatus::Success),
            t(15, 1, NodeStatus::Idle),
        ]
    );
}

#[test]
fn unknown_uids_are_kept() {
    let bytes = handmade_log(SAMPLE_XML, START_US, &[(1, 500, 1), (2, 1, 1)]);
    let log = BtLog::from_bytes(&bytes).unwrap();
    assert_eq!(log.transitions.len(), 2);
    assert!(!log.is_known_uid(Uid(500)));
    assert_eq!(
        log.unknown_uids().into_iter().collect::<Vec<_>>(),
        [Uid(500)]
    );
}

#[test]
fn xml_length_larger_than_file() {
    let mut bytes = write_log(SAMPLE_XML, START_US, &[]);
    bytes[19..23].copy_from_slice(&100_000i32.to_le_bytes());
    match BtLog::from_bytes(&bytes) {
        Err(LogError::XmlLengthTooLarge {
            declared,
            available,
        }) => {
            assert_eq!(declared, 100_000);
            assert_eq!(available, (bytes.len() - 23) as u64);
        }
        other => panic!("unexpected result {other:?}"),
    }
    bytes[19..23].copy_from_slice(&i32::MAX.to_le_bytes());
    assert!(matches!(
        BtLog::from_bytes(&bytes),
        Err(LogError::XmlLengthTooLarge { .. })
    ));
    bytes[19..23].copy_from_slice(&(-1i32).to_le_bytes());
    assert!(matches!(
        BtLog::from_bytes(&bytes),
        Err(LogError::NegativeXmlLength(-1))
    ));
}

#[test]
fn invalid_xml() {
    let bytes = handmade_log("<root><BehaviorTree", START_US, &[]);
    assert!(matches!(
        BtLog::from_bytes(&bytes),
        Err(LogError::InvalidXml(_))
    ));
    let bytes = handmade_log("<root/>", START_US, &[]);
    assert!(matches!(
        BtLog::from_bytes(&bytes),
        Err(LogError::InvalidXml(_))
    ));
    let bytes = handmade_log("", START_US, &[]);
    assert!(matches!(
        BtLog::from_bytes(&bytes),
        Err(LogError::InvalidXml(_))
    ));

    let mut bytes = write_log(SAMPLE_XML, START_US, &[]);
    bytes[30] = 0xFF;
    assert!(matches!(
        BtLog::from_bytes(&bytes),
        Err(LogError::XmlNotUtf8(_))
    ));
}

#[test]
fn errors_have_clear_messages() {
    let message = BtLog::from_bytes(b"hello").unwrap_err().to_string();
    assert!(message.contains("BTCPP4-FileLogger2"), "{message}");
    let mut bytes = write_log(SAMPLE_XML, START_US, &[]);
    bytes[18] = 2;
    let message = BtLog::from_bytes(&bytes).unwrap_err().to_string();
    assert!(message.contains("version 2"), "{message}");
}

#[test]
fn offset_too_large_writes_nothing() {
    let mut writer = BtLogWriter::new(Vec::new(), SAMPLE_XML, START_US).unwrap();
    let batch = [
        t(1, 1, NodeStatus::Running),
        t(MAX_OFFSET_US + 1, 1, NodeStatus::Success),
    ];
    assert!(matches!(
        writer.push_batch(&batch),
        Err(LogError::OffsetTooLarge(_))
    ));
    assert_eq!(writer.records_written(), 0);
    assert_eq!(writer.get_ref().len(), header_len(SAMPLE_XML));
    assert!(matches!(
        writer.push(batch[1]),
        Err(LogError::OffsetTooLarge(_))
    ));
    assert_eq!(writer.get_ref().len(), header_len(SAMPLE_XML));
}

#[test]
fn header_encoding_checks_size() {
    assert!(encode_header("", 0).is_ok());
}

#[test]
fn open_and_create_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.btlog");
    let transitions = sample_transitions();
    let mut writer = BtLogWriter::create(&path, SAMPLE_XML, START_US).unwrap();
    writer.push_batch(&transitions).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let log = BtLog::open(&path).unwrap();
    assert_eq!(log.transitions, transitions);

    let missing = dir.path().join("missing.btlog");
    match BtLog::open(&missing) {
        Err(LogError::Read { path, .. }) => assert_eq!(path, missing),
        other => panic!("unexpected result {other:?}"),
    }
}

#[test]
fn follow_a_growing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("growing.btlog");
    let transitions = sample_transitions();
    let bytes = write_log(SAMPLE_XML, START_US, &transitions);

    let mut append = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap();
    let mut read = std::fs::File::open(&path).unwrap();
    let mut reader = BtLogReader::new();

    // Empty file: nothing yet.
    assert_eq!(reader.read_from(&mut read).unwrap(), 0);
    assert!(reader.header().is_none());

    // Part of the header.
    append.write_all(&bytes[..40]).unwrap();
    assert_eq!(reader.read_from(&mut read).unwrap(), 0);
    assert!(reader.header().is_none());

    // The header and two and a half records.
    let cut = header_len(SAMPLE_XML) + 2 * RECORD_SIZE + 4;
    append.write_all(&bytes[40..cut]).unwrap();
    assert_eq!(reader.read_from(&mut read).unwrap(), 2);
    assert_eq!(reader.header().unwrap().start_time_us, START_US);
    assert_eq!(reader.pending_bytes(), 4);

    // The rest.
    append.write_all(&bytes[cut..]).unwrap();
    assert_eq!(reader.read_from(&mut read).unwrap(), transitions.len() - 2);
    assert_eq!(
        reader.into_log().unwrap(),
        BtLog::from_bytes(&bytes).unwrap()
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn prop_round_trip_is_byte_exact(
        transitions in support::wide_transitions_strategy(200),
        start in any::<i64>(),
    ) {
        let bytes = write_log(SAMPLE_XML, start, &transitions);
        let records: Vec<(u64, u16, u8)> = transitions
            .iter()
            .map(|tr| (tr.offset_us, tr.uid.0, tr.status.as_u8()))
            .collect();
        prop_assert_eq!(&bytes, &handmade_log(SAMPLE_XML, start, &records));
        let log = BtLog::from_bytes(&bytes).unwrap();
        prop_assert_eq!(log.start_time_us, start);
        prop_assert_eq!(&log.transitions, &transitions);
        prop_assert_eq!(log.to_bytes().unwrap(), bytes);
    }

    #[test]
    fn prop_chunked_reading_equals_all_at_once(
        transitions in transitions_strategy(10, 1_000, 60),
        junk in prop::collection::vec(any::<u8>(), 0..40),
        cuts in prop::collection::vec(any::<prop::sample::Index>(), 0..12),
    ) {
        let mut bytes = write_log(SAMPLE_XML, START_US, &transitions);
        bytes.extend_from_slice(&junk);
        let all = BtLog::from_bytes(&bytes).unwrap();
        let mut points: Vec<usize> = cuts.iter().map(|cut| cut.index(bytes.len() + 1)).collect();
        points.push(0);
        points.push(bytes.len());
        points.sort_unstable();
        let mut reader = BtLogReader::new();
        let mut count = 0;
        for pair in points.windows(2) {
            count += reader.push(&bytes[pair[0]..pair[1]]).unwrap();
        }
        prop_assert_eq!(count, all.transitions.len());
        prop_assert_eq!(reader.into_log().unwrap(), all);
    }

    #[test]
    fn prop_arbitrary_bytes_do_not_panic(
        bytes in prop::collection::vec(any::<u8>(), 0..200),
        keep_magic in any::<bool>(),
    ) {
        let mut data = Vec::new();
        if keep_magic {
            data.extend_from_slice(b"BTCPP4-FileLogger2\x01");
        }
        data.extend_from_slice(&bytes);
        let _ = BtLog::from_bytes(&data);
        let mut reader = BtLogReader::new();
        for chunk in data.chunks(7) {
            if reader.push(chunk).is_err() {
                break;
            }
        }
        let _ = reader.to_log();
    }
}

/// A reader that fails in a given way, then gives the data.
struct Flaky<'a> {
    data: &'a [u8],
    interrupt_next: bool,
    fail_at: Option<usize>,
    position: usize,
}

impl std::io::Read for Flaky<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.interrupt_next {
            self.interrupt_next = false;
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        if self.fail_at.is_some_and(|at| self.position >= at) {
            return Err(std::io::Error::other("disk on fire"));
        }
        let end = self.fail_at.unwrap_or(self.data.len()).min(self.data.len());
        let count = 5.min(buf.len()).min(end - self.position);
        buf[..count].copy_from_slice(&self.data[self.position..self.position + count]);
        self.position += count;
        // Interrupt again after each read.
        self.interrupt_next = true;
        Ok(count)
    }
}

#[test]
fn read_from_retries_interrupted_reads() {
    let transitions = sample_transitions();
    let bytes = write_log(SAMPLE_XML, START_US, &transitions);
    let mut source = Flaky {
        data: &bytes,
        interrupt_next: true,
        fail_at: None,
        position: 0,
    };
    let mut reader = BtLogReader::new();
    assert_eq!(reader.read_from(&mut source).unwrap(), transitions.len());
    assert_eq!(
        reader.into_log().unwrap(),
        BtLog::from_bytes(&bytes).unwrap()
    );
}

#[test]
fn read_from_reports_io_errors() {
    let bytes = write_log(SAMPLE_XML, START_US, &sample_transitions());
    let fail_at = header_len(SAMPLE_XML) + 20;
    let mut source = Flaky {
        data: &bytes,
        interrupt_next: false,
        fail_at: Some(fail_at),
        position: 0,
    };
    let mut reader = BtLogReader::new();
    match reader.read_from(&mut source) {
        Err(LogError::Io(err)) => assert_eq!(err.to_string(), "disk on fire"),
        other => panic!("unexpected result {other:?}"),
    }
    // The bytes before the error are kept.
    assert!(reader.header().is_some());
    assert_eq!(reader.transitions().len(), 2);
    assert_eq!(reader.pending_bytes(), 2);
}

#[test]
fn create_error_names_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("no_such_dir").join("run.btlog");
    match BtLogWriter::create(&path, SAMPLE_XML, START_US) {
        Err(LogError::Write { path: reported, .. }) => assert_eq!(reported, path),
        other => panic!("unexpected result {other:?}"),
    }
    let message = BtLogWriter::create(&path, SAMPLE_XML, START_US)
        .unwrap_err()
        .to_string();
    assert!(message.contains("no_such_dir"), "{message}");
}

#[test]
fn header_error_after_the_full_header_keeps_the_reader() {
    // The XML is not valid. The error comes when the header is complete,
    // and the reader stays at the bytes from before.
    let bytes = handmade_log("<root><BehaviorTree", START_US, &[(1, 1, 1)]);
    let header_end = 23 + "<root><BehaviorTree".len() + 8;
    let mut reader = BtLogReader::new();
    reader.push(&bytes[..header_end - 1]).unwrap();
    assert!(matches!(
        reader.push(&bytes[header_end - 1..]),
        Err(LogError::InvalidXml(_))
    ));
    assert_eq!(reader.pending_bytes(), header_end - 1);
    assert!(reader.header().is_none());
    // The same error again, with the same bytes.
    assert!(matches!(
        reader.push(&bytes[header_end - 1..]),
        Err(LogError::InvalidXml(_))
    ));
    assert!(matches!(
        reader.to_log(),
        Err(LogError::TruncatedHeader { .. })
    ));
}

#[test]
fn decode_records_of_a_transitions_reply() {
    use fasttick_log::btlog::decode_records;
    // GET_TRANSITIONS: the same 9-byte records, with an offset from the
    // start of the recording.
    let records = [(0, 3, 1), (1_500, 3, 2), (1_501, 3, 0)];
    let full = handmade_log("", 0, &records);
    let body = &full[header_len("")..];
    let decoded = decode_records(body);
    assert_eq!(
        decoded.transitions,
        vec![
            t(0, 3, NodeStatus::Running),
            t(1_500, 3, NodeStatus::Success),
            t(1_501, 3, NodeStatus::Idle),
        ]
    );
    assert_eq!(decoded.invalid_records, 0);
    assert_eq!(decoded.trailing_bytes, 0);
    assert_eq!(decode_records(&[]).transitions, vec![]);
}
