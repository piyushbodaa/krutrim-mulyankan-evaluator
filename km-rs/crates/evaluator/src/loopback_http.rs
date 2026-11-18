//! Bounded HTTP framing for the loopback workbench and desktop preview.
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};

/// Limit the number of live readers so idle connections cannot create unbounded threads.
pub fn serve(
    listener: std::net::TcpListener,
    handle: impl Fn(std::net::TcpStream) + Send + Sync + 'static,
) {
    let handle = std::sync::Arc::new(handle);
    let mut active: Vec<std::thread::JoinHandle<()>> = Vec::new();
    let timeout = Some(std::time::Duration::from_secs(15));
    for stream in listener.incoming().flatten() {
        active.retain(|worker| !worker.is_finished());
        if active.len() >= 32
            || stream.set_read_timeout(timeout).is_err()
            || stream.set_write_timeout(timeout).is_err()
        {
            continue;
        }
        let handle = handle.clone();
        if let Ok(worker) = std::thread::Builder::new().spawn(move || handle(stream)) {
            active.push(worker);
        }
    }
}

pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

pub fn read_request(input: impl Read, body_limit: usize) -> Result<Request, u16> {
    let mut reader = BufReader::new(input);
    let mut header = (&mut reader).take(32 * 1024);
    let mut line = String::new();
    header.read_line(&mut line).map_err(|_| 400u16)?;
    let mut words = line.split_whitespace();
    let method = words.next().ok_or(400u16)?.to_owned();
    let target = words.next().ok_or(400u16)?.to_owned();
    let mut headers = HashMap::new();
    loop {
        line.clear();
        header.read_line(&mut line).map_err(|_| 400u16)?;
        if line == "\r\n" || line == "\n" {
            break;
        }
        if !line.ends_with('\n') {
            return Err(400);
        }
        let (key, value) = line.split_once(':').ok_or(400u16)?;
        let key = key.to_ascii_lowercase();
        if key == "transfer-encoding"
            || (headers.contains_key(&key)
                && matches!(
                    key.as_str(),
                    "content-length"
                        | "host"
                        | "origin"
                        | "content-type"
                        | "x-km-token"
                        | "x-pilot-token"
                ))
        {
            return Err(400);
        }
        headers.insert(key, value.trim().to_owned());
    }
    let length: usize = match headers.get("content-length") {
        Some(value) => value.parse().map_err(|_| 400u16)?,
        None => 0,
    };
    if length > body_limit {
        return Err(413);
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).map_err(|_| 400u16)?;
    Ok(Request {
        method,
        target,
        headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respects_body_limits_and_requires_complete_frames() {
        let frame = b"POST /rpc HTTP/1.1\r\nContent-Length: 2\r\n\r\n{}";
        assert_eq!(read_request(frame.as_slice(), 2).unwrap().body, b"{}");
        assert_eq!(read_request(frame.as_slice(), 1).err(), Some(413));
        assert_eq!(read_request(&frame[..frame.len() - 1], 2).err(), Some(400));
    }
}
