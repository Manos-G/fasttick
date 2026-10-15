//! Live tests of the executor with its BT.CPP publisher.
//!
//! The tests use a minimal ZeroMQ client: ZMTP 3.0 with the NULL security
//! mechanism, and only the REQ and SUB socket types. It uses std only,
//! because this crate has no ZeroMQ dependency. The framing follows the
//! public ZMTP 3.0 specification (rfc.zeromq.org, spec 23). The message
//! layout comes from BT.CPP: the protocol header (headers, hook JSON) and
//! the publisher source (requests, replies and the "breakpoint reached"
//! notification).
//!
//! Each test skips cleanly when BT.CPP 4.x is not found.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::time::{Duration, Instant};

use fasttick_testkit::{Executor, ExecutorOptions, Harness, free_port_pair, require_harness};

/// Time limit for one socket read or write.
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// ZMTP frame flags.
const FLAG_MORE: u8 = 0x01;
const FLAG_LONG: u8 = 0x02;
const FLAG_COMMAND: u8 = 0x04;

/// Largest frame that the client accepts (the full tree XML is small).
const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Protocol ID in the request header (`kProtocolID` in the protocol header).
const PROTOCOL_ID: u8 = 2;

/// Size of the reply header: request header (6 bytes) and tree UUID (16).
const REPLY_HEADER_SIZE: usize = 22;

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// One ZMTP 3.0 connection.
struct ZmtpSocket {
    stream: TcpStream,
}

impl ZmtpSocket {
    /// Connects to 127.0.0.1:`port` and does the handshake with the given
    /// socket type ("REQ" or "SUB").
    fn connect(port: u16, socket_type: &str) -> io::Result<ZmtpSocket> {
        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        stream.set_nodelay(true)?;
        let mut socket = ZmtpSocket { stream };
        socket.handshake(socket_type)?;
        Ok(socket)
    }

    fn handshake(&mut self, socket_type: &str) -> io::Result<()> {
        // Greeting: signature (10 bytes), version 3.0, mechanism (20 bytes),
        // as-server (1 byte), filler (31 bytes).
        let mut greeting = [0_u8; 64];
        greeting[0] = 0xFF;
        greeting[8] = 0x01;
        greeting[9] = 0x7F;
        greeting[10] = 3;
        greeting[11] = 0;
        greeting[12..16].copy_from_slice(b"NULL");
        self.stream.write_all(&greeting)?;

        let mut peer = [0_u8; 64];
        self.stream.read_exact(&mut peer)?;
        if peer[0] != 0xFF || peer[9] != 0x7F || peer[10] < 3 {
            return Err(invalid(format!("bad ZMTP greeting: {:02x?}", &peer[..12])));
        }
        if &peer[12..16] != b"NULL" {
            return Err(invalid("the peer does not use the NULL mechanism"));
        }

        // READY command with the Socket-Type property.
        let mut ready = vec![5_u8];
        ready.extend_from_slice(b"READY");
        ready.push(11);
        ready.extend_from_slice(b"Socket-Type");
        let length = u32::try_from(socket_type.len()).map_err(|_| invalid("long type"))?;
        ready.extend_from_slice(&length.to_be_bytes());
        ready.extend_from_slice(socket_type.as_bytes());
        self.write_frame(FLAG_COMMAND, &ready)?;

        let (flags, body) = self.read_frame()?;
        if flags & FLAG_COMMAND == 0 || !body.starts_with(b"\x05READY") {
            return Err(invalid("the peer did not send READY"));
        }
        Ok(())
    }

    fn write_frame(&mut self, flags: u8, body: &[u8]) -> io::Result<()> {
        let mut frame = Vec::with_capacity(body.len() + 9);
        match u8::try_from(body.len()) {
            Ok(size) => {
                frame.push(flags);
                frame.push(size);
            }
            Err(_) => {
                frame.push(flags | FLAG_LONG);
                frame.extend_from_slice(&(body.len() as u64).to_be_bytes());
            }
        }
        frame.extend_from_slice(body);
        self.stream.write_all(&frame)
    }

    fn read_frame(&mut self) -> io::Result<(u8, Vec<u8>)> {
        let mut flags = [0_u8; 1];
        self.stream.read_exact(&mut flags)?;
        let size = if flags[0] & FLAG_LONG != 0 {
            let mut size = [0_u8; 8];
            self.stream.read_exact(&mut size)?;
            u64::from_be_bytes(size)
        } else {
            let mut size = [0_u8; 1];
            self.stream.read_exact(&mut size)?;
            u64::from(size[0])
        };
        let size = usize::try_from(size)
            .ok()
            .filter(|size| *size <= MAX_FRAME)
            .ok_or_else(|| invalid(format!("frame too large: {size} bytes")))?;
        let mut body = vec![0_u8; size];
        self.stream.read_exact(&mut body)?;
        Ok((flags[0], body))
    }

    fn send_message(&mut self, parts: &[&[u8]]) -> io::Result<()> {
        for (index, part) in parts.iter().enumerate() {
            let flags = if index + 1 < parts.len() {
                FLAG_MORE
            } else {
                0
            };
            self.write_frame(flags, part)?;
        }
        Ok(())
    }

    /// Reads one message (all its parts). Commands are ignored.
    fn read_message(&mut self) -> io::Result<Vec<Vec<u8>>> {
        let mut parts = Vec::new();
        loop {
            let (flags, body) = self.read_frame()?;
            if flags & FLAG_COMMAND != 0 {
                continue;
            }
            parts.push(body);
            if flags & FLAG_MORE == 0 {
                return Ok(parts);
            }
        }
    }

    /// REQ: sends the request with the empty delimiter frame and reads the
    /// reply without the delimiter.
    fn request(&mut self, parts: &[&[u8]]) -> io::Result<Vec<Vec<u8>>> {
        let mut message: Vec<&[u8]> = vec![&[]];
        message.extend_from_slice(parts);
        self.send_message(&message)?;
        let mut reply = self.read_message()?;
        if reply.first().is_none_or(|delimiter| !delimiter.is_empty()) {
            return Err(invalid("the reply has no empty delimiter frame"));
        }
        reply.remove(0);
        Ok(reply)
    }

    /// SUB: subscribes to all messages (ZMTP 3.0 subscription message).
    fn subscribe_all(&mut self) -> io::Result<()> {
        self.write_frame(0, &[0x01])
    }
}

/// A request header: protocol, type, unique ID (u32, little-endian), as
/// `SerializeHeader(const RequestHeader&)` in the protocol header writes it.
fn request_header(request_type: u8, id: u32) -> Vec<u8> {
    let mut header = vec![PROTOCOL_ID, request_type];
    header.extend_from_slice(&id.to_le_bytes());
    header
}

/// Sends a request and checks the reply header. Gives the other parts.
fn call(socket: &mut ZmtpSocket, request_type: u8, id: u32, body: Option<&[u8]>) -> Vec<Vec<u8>> {
    let header = request_header(request_type, id);
    let mut parts: Vec<&[u8]> = vec![&header];
    parts.extend(body);
    let mut reply = socket.request(&parts).unwrap();
    assert!(!reply.is_empty(), "empty reply");
    let reply_header = reply.remove(0);
    assert_ne!(
        reply_header,
        b"error",
        "error reply: {:?}",
        reply
            .first()
            .map(|text| String::from_utf8_lossy(text).into_owned())
    );
    assert_eq!(reply_header.len(), REPLY_HEADER_SIZE);
    // The reply repeats the request header (ReplyHeader in the protocol header).
    assert_eq!(&reply_header[..6], &header[..]);
    reply
}

/// Finds the `_uid` of the node with the given name in the runtime XML.
fn uid_of(xml: &str, name: &str) -> u16 {
    let start = xml
        .find(&format!("name=\"{name}\""))
        .unwrap_or_else(|| panic!("no node {name}"));
    let rest = &xml[start..];
    let uid_start = rest.find("_uid=\"").unwrap() + "_uid=\"".len();
    let uid_end = rest[uid_start..].find('"').unwrap() + uid_start;
    rest[uid_start..uid_end].parse().unwrap()
}

fn start_long_running(heartbeat_ms: u64) -> Option<(Executor, u16)> {
    let harness = require_harness!(None);
    let port = free_port_pair();
    let mut options = ExecutorOptions::new(harness.tree("long_running.xml"));
    options.port = port;
    options.heartbeat_ms = Some(heartbeat_ms);
    let executor = Executor::spawn_with(harness, options).unwrap();
    Some((executor, port))
}

#[test]
fn publisher_answers_fulltree_and_status() {
    let Some((mut executor, port)) = start_long_running(5000) else {
        return;
    };
    let mut client = ZmtpSocket::connect(port, "REQ").unwrap();

    let reply = call(&mut client, b'T', 7, None);
    assert_eq!(reply.len(), 1);
    let xml = String::from_utf8(reply[0].clone()).unwrap();
    assert!(xml.contains("<BehaviorTree ID=\"LongRunning\""), "{xml}");
    assert_eq!(uid_of(&xml, "long_sequence"), 1);
    let node_count = xml.matches("_uid=\"").count();
    assert!(node_count > 10, "{node_count} nodes");

    // STATUS: 3 bytes for each node, UID (u16 LE) then the status.
    let reply = call(&mut client, b'S', 8, None);
    assert_eq!(reply.len(), 1);
    assert_eq!(reply[0].len(), 3 * node_count);
    let uids: Vec<u16> = reply[0]
        .as_chunks::<3>()
        .0
        .iter()
        .map(|entry| u16::from_le_bytes([entry[0], entry[1]]))
        .collect();
    let mut sorted = uids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), node_count, "UIDs are unique: {uids:?}");

    // A header with the wrong size gives the 2-part error reply.
    client.send_message(&[&[], b"bad"]).unwrap();
    let reply = client.read_message().unwrap();
    assert_eq!(reply.len(), 3, "{reply:?}");
    assert_eq!(reply[1], b"error");

    let status = executor.stop().unwrap();
    assert!(status.success(), "{status}");
}

/// True if CMake found libzmq for the harness build. Without libzmq, the
/// executor cannot release a blocked breakpoint (see test_harness/README.md).
fn harness_has_libzmq(harness: &Harness) -> bool {
    let cache = std::fs::read_to_string(harness.build_dir.join("CMakeCache.txt")).unwrap();
    ["FASTTICK_ZMQ_INCLUDE_DIR:", "FASTTICK_ZMQ_LIBRARY:"]
        .iter()
        .all(|key| {
            cache.lines().any(|line| {
                line.starts_with(key)
                    && line.split_once('=').is_some_and(|(_, value)| {
                        !value.is_empty() && !value.ends_with("-NOTFOUND")
                    })
            })
        })
}

#[test]
fn sigterm_releases_a_blocked_breakpoint() {
    let harness = require_harness!();
    if !harness_has_libzmq(harness) {
        eprintln!("SKIPPED: the harness was built without libzmq");
        return;
    }
    // A long heartbeat: the publisher keeps the hook enabled without polls.
    // Only the executor's own release can then unblock the tick.
    let Some((mut executor, port)) = start_long_running(120_000) else {
        return;
    };
    let mut subscriber = ZmtpSocket::connect(port + 1, "SUB").unwrap();
    subscriber.subscribe_all().unwrap();
    let mut client = ZmtpSocket::connect(port, "REQ").unwrap();

    let reply = call(&mut client, b'T', 1, None);
    let xml = String::from_utf8(reply[0].clone()).unwrap();
    let uid = uid_of(&xml, "say_cycle");
    // Give the publisher time to register the subscription.
    std::thread::sleep(Duration::from_millis(200));

    // HOOK_INSERT: an interactive breakpoint before the node. The field
    // names come from to_json(Hook) in the protocol header.
    let hook = format!(
        "{{\"enabled\":true,\"uid\":{uid},\"mode\":0,\"once\":false,\
         \"desired_status\":\"SKIPPED\",\"position\":0}}"
    );
    let reply = call(&mut client, b'I', 2, Some(hook.as_bytes()));
    assert!(reply.is_empty(), "{reply:?}");

    // The tick reaches the breakpoint and blocks. The publisher sends
    // a request header of type BREAKPOINT_REACHED ('N') and the UID text.
    let notification = subscriber.read_message().unwrap();
    assert_eq!(notification.len(), 2, "{notification:?}");
    assert_eq!(notification[0].len(), 6);
    assert_eq!(notification[0][0], PROTOCOL_ID);
    assert_eq!(notification[0][1], b'N');
    assert_eq!(notification[1], uid.to_string().as_bytes());

    // The tick stays blocked: the process runs and writes no result.
    std::thread::sleep(Duration::from_millis(300));
    assert!(executor.is_running().unwrap());

    let start = Instant::now();
    let status = executor.stop().unwrap();
    let elapsed = start.elapsed();
    assert!(status.success(), "{status}; stderr:\n{}", executor.stderr());
    assert!(
        elapsed < Duration::from_secs(5),
        "the stop took {elapsed:?}; the breakpoint was not released"
    );
    assert!(
        executor
            .stdout_lines()
            .iter()
            .any(|line| line == "STOPPED signal=15"),
        "{:?}",
        executor.stdout_lines()
    );
}
