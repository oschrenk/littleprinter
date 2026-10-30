//! Relay the Bridge daemon's websocket to a server, and log every frame.
//!
//! Bytes are forwarded before they are parsed, so a parser bug can spoil the
//! log but not the connection. Only the `Host` header is rewritten, because
//! the daemon sends the proxy's address there and the server routes on it.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_HEAD: usize = 16 * 1024;
const MAX_CAPTURE: usize = 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

type Log = Arc<Mutex<dyn Write + Send>>;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let payloads = args.iter().any(|a| a == "--payloads");
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let [listen, upstream] = positional[..] else {
        eprintln!("usage: lp-proxy <listen-addr:port> <upstream-host[:port]> [--payloads]");
        std::process::exit(2);
    };
    if payloads {
        eprintln!("lp-proxy: the log will hold link keys in cleartext, so do not commit it");
    }
    let listener = TcpListener::bind(listen).unwrap_or_else(|e| {
        eprintln!("lp-proxy: bind {listen}: {e}");
        std::process::exit(1);
    });
    serve(
        listener,
        upstream.clone(),
        payloads,
        Arc::new(Mutex::new(io::stdout())),
    );
}

fn serve(listener: TcpListener, upstream: String, payloads: bool, log: Log) {
    let mut conn = 0;
    for client in listener.incoming() {
        let Ok(client) = client else { continue };
        conn += 1;
        let (upstream, log) = (upstream.clone(), log.clone());
        std::thread::spawn(move || {
            if let Err(e) = handle(client, &upstream, payloads, conn, &log) {
                event(&log, conn, "error", &e.to_string());
            }
        });
    }
}

fn handle(
    mut client: TcpStream,
    upstream: &str,
    payloads: bool,
    conn: u64,
    log: &Log,
) -> io::Result<()> {
    client.set_nodelay(true)?;
    let (head, rest) = read_head(&mut client)?;
    let addr = upstream_addr(upstream);
    let addr = addr
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no upstream address"))?;
    let mut server = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    server.set_nodelay(true)?;
    event(log, conn, "connect", &addr.to_string());

    server.write_all(&rewrite_host(&head, upstream))?;
    server.write_all(&rest)?;
    let (reply, reply_rest) = read_head(&mut server)?;
    client.write_all(&reply)?;
    client.write_all(&reply_rest)?;
    let status = reply.split(|&b| b == b'\r').next().unwrap_or_default();
    event(log, conn, "status", &String::from_utf8_lossy(status));
    let is_websocket = status.windows(5).any(|w| w == b" 101 ");

    let mut up = Pump::new("up", conn, payloads, is_websocket, log.clone());
    let mut down = Pump::new("down", conn, payloads, is_websocket, log.clone());
    up.observe(&rest);
    down.observe(&reply_rest);

    let (client2, server2) = (client.try_clone()?, server.try_clone()?);
    let t = std::thread::spawn(move || up.run(client2, server2));
    down.run(server, client);
    let _ = t.join();
    event(log, conn, "close", "");
    Ok(())
}

fn upstream_addr(upstream: &str) -> String {
    if upstream.contains(':') {
        upstream.to_string()
    } else {
        format!("{upstream}:80")
    }
}

fn read_head(stream: &mut TcpStream) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(end + 4);
            return Ok((buf, rest));
        }
        if buf.len() > MAX_HEAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "http head too long",
            ));
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn rewrite_host(head: &[u8], host: &str) -> Vec<u8> {
    let host = host.strip_suffix(":80").unwrap_or(host);
    let text = String::from_utf8_lossy(head);
    let lines: Vec<String> = text
        .split("\r\n")
        .map(|l| {
            if l.to_ascii_lowercase().starts_with("host:") {
                format!("Host: {host}")
            } else {
                l.to_string()
            }
        })
        .collect();
    lines.join("\r\n").into_bytes()
}

struct Pump {
    dir: &'static str,
    conn: u64,
    log: Log,
    parser: Option<Parser>,
}

impl Pump {
    fn new(dir: &'static str, conn: u64, payloads: bool, is_websocket: bool, log: Log) -> Self {
        let parser = is_websocket.then(|| Parser::new(payloads));
        Pump {
            dir,
            conn,
            log,
            parser,
        }
    }

    fn observe(&mut self, data: &[u8]) {
        let Some(parser) = self.parser.as_mut() else {
            return;
        };
        for frame in parser.feed(data) {
            let line = frame.to_json(self.conn, self.dir);
            write_line(&self.log, &line);
        }
    }

    fn run(&mut self, mut from: TcpStream, mut to: TcpStream) {
        let mut buf = [0u8; 8192];
        loop {
            let n = match from.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            if to.write_all(&buf[..n]).is_err() {
                break;
            }
            self.observe(&buf[..n]);
        }
        let _ = from.shutdown(Shutdown::Both);
        let _ = to.shutdown(Shutdown::Both);
    }
}

#[derive(Debug, PartialEq)]
struct Frame {
    fin: bool,
    opcode: u8,
    len: u64,
    payload: Option<Vec<u8>>,
}

impl Frame {
    fn to_json(&self, conn: u64, dir: &str) -> String {
        let op = match self.opcode {
            0 => "continuation",
            1 => "text",
            2 => "binary",
            8 => "close",
            9 => "ping",
            10 => "pong",
            _ => "reserved",
        };
        let mut s = format!(
            "{{\"t\":{:.3},\"conn\":{conn},\"dir\":\"{dir}\",\"op\":\"{op}\",\"fin\":{},\"len\":{}",
            now(),
            self.fin,
            self.len
        );
        if let Some(p) = &self.payload {
            if self.opcode == 1 || self.opcode == 0 {
                s += &format!(
                    ",\"payload\":\"{}\"",
                    json_escape(&String::from_utf8_lossy(p))
                );
            } else {
                let hex: String = p.iter().map(|b| format!("{b:02x}")).collect();
                s += &format!(",\"payload_hex\":\"{hex}\"");
            }
            if (p.len() as u64) < self.len {
                s += ",\"truncated\":true";
            }
        }
        s + "}"
    }
}

/// An incremental RFC 6455 frame parser that never buffers more than
/// `MAX_CAPTURE` of a payload, and nothing at all without `--payloads`.
struct Parser {
    capture: bool,
    head: Vec<u8>,
    remaining: u64,
    in_payload: bool,
    mask: Option<[u8; 4]>,
    index: usize,
    fin: bool,
    opcode: u8,
    len: u64,
    payload: Vec<u8>,
}

impl Parser {
    fn new(capture: bool) -> Self {
        Parser {
            capture,
            head: Vec::with_capacity(14),
            remaining: 0,
            in_payload: false,
            mask: None,
            index: 0,
            fin: false,
            opcode: 0,
            len: 0,
            payload: Vec::new(),
        }
    }

    fn feed(&mut self, mut data: &[u8]) -> Vec<Frame> {
        let mut frames = Vec::new();
        while !data.is_empty() {
            if self.in_payload {
                let n = (self.remaining.min(data.len() as u64)) as usize;
                self.take_payload(&data[..n]);
                data = &data[n..];
            } else {
                self.head.push(data[0]);
                data = &data[1..];
                self.try_finish_head();
            }
            if self.in_payload && self.remaining == 0 {
                frames.push(self.finish());
            }
        }
        frames
    }

    fn head_size(&self) -> Option<usize> {
        let second = *self.head.get(1)?;
        let extended = match second & 0x7F {
            126 => 2,
            127 => 8,
            _ => 0,
        };
        Some(2 + extended + if second & 0x80 != 0 { 4 } else { 0 })
    }

    fn try_finish_head(&mut self) {
        if self.head_size() != Some(self.head.len()) {
            return;
        }
        let (first, second) = (self.head[0], self.head[1]);
        let (len, rest) = match second & 0x7F {
            126 => (u16::from_be_bytes([self.head[2], self.head[3]]) as u64, 4),
            127 => {
                let mut b = [0u8; 8];
                b.copy_from_slice(&self.head[2..10]);
                (u64::from_be_bytes(b), 10)
            }
            n => (n as u64, 2),
        };
        self.mask = (second & 0x80 != 0).then(|| {
            let mut m = [0u8; 4];
            m.copy_from_slice(&self.head[rest..rest + 4]);
            m
        });
        self.fin = first & 0x80 != 0;
        self.opcode = first & 0x0F;
        self.len = len;
        self.remaining = len;
        self.index = 0;
        self.in_payload = true;
        self.head.clear();
    }

    fn take_payload(&mut self, data: &[u8]) {
        self.remaining -= data.len() as u64;
        if self.capture {
            for &b in data {
                if self.payload.len() < MAX_CAPTURE {
                    let m = self.mask.map_or(0, |m| m[self.index % 4]);
                    self.payload.push(b ^ m);
                }
                self.index += 1;
            }
        }
    }

    fn finish(&mut self) -> Frame {
        self.in_payload = false;
        Frame {
            fin: self.fin,
            opcode: self.opcode,
            len: self.len,
            payload: self.capture.then(|| std::mem::take(&mut self.payload)),
        }
    }
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            '\n' => out += "\\n",
            '\r' => out += "\\r",
            '\t' => out += "\\t",
            c if (c as u32) < 0x20 => out += &format!("\\u{:04x}", c as u32),
            c => out.push(c),
        }
    }
    out
}

fn event(log: &Log, conn: u64, kind: &str, detail: &str) {
    let line = format!(
        "{{\"t\":{:.3},\"conn\":{conn},\"event\":\"{kind}\",\"detail\":\"{}\"}}",
        now(),
        json_escape(detail)
    );
    write_line(log, &line);
}

fn write_line(log: &Log, line: &str) {
    if let Ok(mut w) = log.lock() {
        let _ = writeln!(w, "{line}");
        let _ = w.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn masked_text(text: &[u8], mask: [u8; 4]) -> Vec<u8> {
        let mut f = vec![0x81, 0x80 | text.len() as u8];
        f.extend_from_slice(&mask);
        f.extend(text.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        f
    }

    #[test]
    fn parses_a_masked_frame_split_at_every_byte() {
        let f = masked_text(b"hello", [1, 2, 3, 4]);
        for split in 0..=f.len() {
            let mut p = Parser::new(true);
            let mut frames = p.feed(&f[..split]);
            frames.extend(p.feed(&f[split..]));
            assert_eq!(frames.len(), 1, "split {split}");
            assert_eq!(frames[0].payload.as_deref(), Some(&b"hello"[..]));
            assert!(frames[0].fin);
            assert_eq!(frames[0].opcode, 1);
        }
    }

    #[test]
    fn parses_extended_lengths_and_consecutive_frames() {
        let mut data = vec![0x82, 126, 0x01, 0x00];
        data.extend(vec![7u8; 256]);
        data.extend([0x82, 127, 0, 0, 0, 0, 0, 1, 0, 0]);
        data.extend(vec![8u8; 65536]);
        data.extend([0x89, 0]);
        let frames = Parser::new(true).feed(&data);
        assert_eq!(frames.len(), 3);
        assert_eq!((frames[0].len, frames[0].opcode), (256, 2));
        assert_eq!(frames[1].len, 65536);
        assert_eq!((frames[2].len, frames[2].opcode), (0, 9));
    }

    #[test]
    fn keeps_no_payload_without_capture() {
        let frames = Parser::new(false).feed(&masked_text(b"secret", [9, 9, 9, 9]));
        assert_eq!(frames[0].payload, None);
        assert_eq!(frames[0].len, 6);
        assert!(!frames[0].to_json(1, "up").contains("payload"));
    }

    #[test]
    fn truncates_a_payload_over_the_cap() {
        let len = MAX_CAPTURE as u64 + 5;
        let mut data = vec![0x82, 127];
        data.extend(len.to_be_bytes());
        data.extend(vec![1u8; len as usize]);
        let frames = Parser::new(true).feed(&data);
        assert_eq!(frames[0].payload.as_ref().unwrap().len(), MAX_CAPTURE);
        assert!(frames[0].to_json(1, "down").contains("\"truncated\":true"));
    }

    #[test]
    fn rewrites_only_the_host_header() {
        let head = b"GET /x HTTP/1.1\r\nhost: 10.0.0.1:8080\r\nOrigin: ws://a\r\n\r\n";
        let out = rewrite_host(head, "example.org:80");
        assert_eq!(
            out,
            b"GET /x HTTP/1.1\r\nHost: example.org\r\nOrigin: ws://a\r\n\r\n"
        );
    }

    #[test]
    fn escapes_json() {
        assert_eq!(json_escape("a\"b\\c\n\u{1}"), "a\\\"b\\\\c\\n\\u0001");
    }

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn relays_bytes_unchanged_and_logs_frames() {
        const REPLY: &[u8] = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\r\n";
        let up_frame = masked_text(b"{\"type\":\"BridgeEvent\"}", [5, 6, 7, 8]);
        let down_frame = [0x81, 2, b'h', b'i'];

        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let upstream = server.local_addr().unwrap().to_string();
        let expected_up = up_frame.clone();
        let expected_host = format!("Host: {upstream}\r\n");
        let fake = std::thread::spawn(move || {
            let (mut s, _) = server.accept().unwrap();
            let (head, rest) = read_head(&mut s).unwrap();
            assert!(String::from_utf8_lossy(&head).contains(&expected_host));
            s.write_all(REPLY).unwrap();
            s.write_all(&down_frame).unwrap();
            let mut got = rest;
            while got.len() < expected_up.len() {
                let mut b = [0u8; 64];
                let n = s.read(&mut b).unwrap();
                got.extend_from_slice(&b[..n]);
            }
            assert_eq!(got, expected_up);
        });

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let sink = Sink::default();
        let log: Log = Arc::new(Mutex::new(sink.clone()));
        std::thread::spawn(move || serve(listener, upstream, true, log));

        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(b"GET /api/v1/connection HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .unwrap();
        let (reply, mut rest) = read_head(&mut c).unwrap();
        assert_eq!(reply, REPLY);
        while rest.len() < down_frame.len() {
            let mut b = [0u8; 16];
            let n = c.read(&mut b).unwrap();
            rest.extend_from_slice(&b[..n]);
        }
        assert_eq!(rest, down_frame);
        c.write_all(&up_frame).unwrap();
        fake.join().unwrap();
        drop(c);

        let mut text = String::new();
        for _ in 0..100 {
            text = String::from_utf8_lossy(&sink.0.lock().unwrap()).to_string();
            if text.contains("\"event\":\"close\"") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(text.contains("\"dir\":\"up\",\"op\":\"text\""), "{text}");
        assert!(text.contains("BridgeEvent"), "{text}");
        assert!(text.contains("\"dir\":\"down\",\"op\":\"text\""), "{text}");
        assert!(text.contains("\"payload\":\"hi\""), "{text}");
        assert!(text.contains(" 101 "), "{text}");
        assert!(text.contains("\"event\":\"close\""), "{text}");
    }

    #[test]
    fn passes_a_non_websocket_reply_through() {
        const REPLY: &[u8] = b"HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\n\r\nno";
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let upstream = server.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            let (mut s, _) = server.accept().unwrap();
            read_head(&mut s).unwrap();
            s.write_all(REPLY).unwrap();
        });
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let log: Log = Arc::new(Mutex::new(Sink::default()));
        std::thread::spawn(move || serve(listener, upstream, false, log));

        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut got = Vec::new();
        c.read_to_end(&mut got).unwrap();
        assert_eq!(got, REPLY);
    }
}
