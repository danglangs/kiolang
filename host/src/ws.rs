//! The `ws` capability (WS1): one WebSocket event, and the sends it makes, for one run. The
//! connection itself lives in listen.rs; a run only sees this event and buffers its sends.
use crate::kio::caps::ws::{HostWsMessage, HostWsSend, WsErr, WsMessage, WsSend};
use crate::{ok, outcome, State};
use std::sync::{Arc, Mutex, PoisonError};
use wasmtime::component::Resource;
use wasmtime::Result;

/// Most bytes one run may send; the send past it is `too-large`.
pub const MAX_SENT: usize = 1 << 20;

/// One event on a connection, granted to a run as its `WsMessage`.
#[derive(Clone, Debug)]
pub struct WsEvent {
    /// "open", "text", "binary" or "close".
    pub kind: &'static str,
    pub conn: u64,
    pub data: Vec<u8>,
    /// The connection's upgrade request, shared by its events.
    pub upgrade: Arc<Upgrade>,
}

/// What a program sees of a connection's upgrade request (WS1b).
#[derive(Debug, Default)]
pub struct Upgrade {
    /// "/path?query" as sent.
    pub target: String,
    /// Lowercase names, in order, values UTF-8.
    pub headers: Vec<(String, String)>,
}

impl Upgrade {
    /// The first value of header `name`, in any case.
    pub fn header(&self, name: &str) -> Option<String> {
        let (_, v) = self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name))?;
        Some(v.clone())
    }
}

/// A frame a run asked to send, written after the run ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WsFrame {
    Text(String),
    Binary(Vec<u8>),
    Close(u16),
}

/// What a run's `WsSend` collected; shared so the host can take it after the run.
#[derive(Debug, Default)]
pub struct Outbox {
    pub frames: Vec<WsFrame>,
    bytes: usize,
    /// The connection is closed, or this run already sent `close`.
    pub closed: bool,
}

pub type Sends = Arc<Mutex<Outbox>>;

/// Close codes a server may send (RFC 6455 7.4): not 1004-1006 or 1015, which never go on the
/// wire, nor 1014-2999, which the protocol reserves and clients such as tungstenite refuse.
fn valid_close(code: u16) -> bool {
    matches!(code, 1000..=1003 | 1007..=1013 | 3000..=4999)
}

impl crate::kio::caps::ws::Host for State {}

impl HostWsMessage for State {
    fn kind(&mut self, m: Resource<WsMessage>) -> Result<String> {
        self.call(m.rep(), "kind", "kind", |s| Ok(s.event(&m)?.kind.to_string()), ok)
    }

    fn conn(&mut self, m: Resource<WsMessage>) -> Result<u64> {
        self.call(m.rep(), "conn", "conn", |s| Ok(s.event(&m)?.conn), ok)
    }

    fn data(&mut self, m: Resource<WsMessage>) -> Result<Vec<u8>> {
        let data = |s: &mut Self| {
            let data = s.event(&m)?.data.clone();
            s.spend_io(data.len())?;
            Ok(data)
        };
        self.call(m.rep(), "data", "data", data, ok)
    }

    fn target(&mut self, m: Resource<WsMessage>) -> Result<String> {
        self.call(
            m.rep(),
            "target",
            "target",
            |s| Ok(s.event(&m)?.upgrade.target.clone()),
            ok,
        )
    }

    /// The audit log gets the name, never the value.
    fn header(&mut self, m: Resource<WsMessage>, name: String) -> Result<Option<String>> {
        let header = |s: &mut Self| Ok(s.event(&m)?.upgrade.header(&name));
        self.call(m.rep(), "header", &name, header, ok)
    }

    fn drop(&mut self, m: Resource<WsMessage>) -> Result<()> {
        let _ = self.table.delete(Resource::<WsEvent>::new_own(m.rep()));
        Ok(())
    }
}

impl HostWsSend for State {
    fn text(&mut self, w: Resource<WsSend>, s: String) -> Result<Result<(), WsErr>> {
        self.call(
            w.rep(),
            "text",
            "text",
            |st| st.push_frame(&w, WsFrame::Text(s)),
            outcome,
        )
    }

    fn binary(&mut self, w: Resource<WsSend>, b: Vec<u8>) -> Result<Result<(), WsErr>> {
        self.call(
            w.rep(),
            "binary",
            "binary",
            |st| st.push_frame(&w, WsFrame::Binary(b)),
            outcome,
        )
    }

    fn close(&mut self, w: Resource<WsSend>, code: u16) -> Result<Result<(), WsErr>> {
        self.call(
            w.rep(),
            "close",
            "close",
            |st| st.push_frame(&w, WsFrame::Close(code)),
            outcome,
        )
    }

    fn drop(&mut self, w: Resource<WsSend>) -> Result<()> {
        let _ = self.table.delete(Resource::<Sends>::new_own(w.rep()));
        Ok(())
    }
}

impl State {
    fn event(&self, m: &Resource<WsMessage>) -> Result<&WsEvent> {
        Ok(self.table.get(&Resource::<WsEvent>::new_borrow(m.rep()))?)
    }

    /// Buffer `frame`, or say why not. A refused frame is not recorded.
    fn push_frame(&mut self, w: &Resource<WsSend>, frame: WsFrame) -> Result<Result<(), WsErr>> {
        let len = match &frame {
            WsFrame::Text(s) => s.len(),
            WsFrame::Binary(b) => b.len(),
            WsFrame::Close(_) => 0,
        };
        self.spend_io(len)?;
        let sends = self.table.get(&Resource::<Sends>::new_borrow(w.rep()))?.clone();
        let mut out = sends.lock().unwrap_or_else(PoisonError::into_inner);
        if out.closed {
            return Ok(Err(WsErr::Closed));
        }
        match frame {
            WsFrame::Close(code) if !valid_close(code) => return Ok(Err(WsErr::Invalid)),
            WsFrame::Close(_) => out.closed = true,
            _ if out.bytes + len > MAX_SENT => return Ok(Err(WsErr::TooLarge)),
            _ => out.bytes += len,
        }
        out.frames.push(frame);
        Ok(Ok(()))
    }
}
