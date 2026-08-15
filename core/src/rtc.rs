use std::collections::VecDeque;
use std::net::SocketAddr;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::slice;
use std::str;
use std::time::Instant;
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::channel::{ChannelConfig, ChannelId, Reliability};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};

pub const ENGINE: &str = "str0m/0.21.0";
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 64 * 1024;
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
pub const MAX_SDP_BYTES: usize = 256 * 1024;

#[derive(Debug)]
pub enum RtcOutput {
    Timeout(Instant),
    Transmit {
        source: SocketAddr,
        destination: SocketAddr,
        contents: Vec<u8>,
    },
    Event(Event),
}

#[repr(C)]
pub struct RtcBuffer {
    pub data: *mut u8,
    pub len: usize,
}

#[repr(C)]
pub struct RtcPoll {
    pub kind: u32,
    pub timeout_millis: u64,
    pub source: RtcBuffer,
    pub destination: RtcBuffer,
    pub payload: RtcBuffer,
    pub binary: u32,
}

fn owned_buffer(bytes: Vec<u8>) -> RtcBuffer {
    let boxed = bytes.into_boxed_slice();
    let len = boxed.len();
    RtcBuffer {
        data: Box::into_raw(boxed) as *mut u8,
        len,
    }
}

fn empty_buffer() -> RtcBuffer {
    RtcBuffer {
        data: ptr::null_mut(),
        len: 0,
    }
}

/// One worker-owned, Sans-I/O WebRTC engine.
///
/// Socket readiness, timeouts and every `poll_output` drain remain the host
/// provider's responsibility. Hara sees only its opaque handle via
/// `hoplite.rtc`, never this state machine or its channel identifier.
pub struct RtcEngine {
    rtc: Rtc,
    label: String,
    channel: Option<ChannelId>,
    pending_offer: Option<SdpPendingOffer>,
    max_message_bytes: usize,
    outputs: VecDeque<RtcOutput>,
}

unsafe fn bytes<'a>(data: *const u8, len: usize) -> Result<&'a [u8], String> {
    if data.is_null() {
        return if len == 0 {
            Ok(&[])
        } else {
            Err("null RTC input".into())
        };
    }
    Ok(slice::from_raw_parts(data, len))
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_rtc_engine_new(
    label: *const u8,
    label_len: usize,
    max_message_bytes: usize,
) -> *mut RtcEngine {
    catch_unwind(AssertUnwindSafe(|| {
        let label = str::from_utf8(bytes(label, label_len)?).map_err(|error| error.to_string())?;
        RtcEngine::new(Instant::now(), label, max_message_bytes).map(Box::new)
    }))
    .ok()
    .and_then(Result::ok)
    .map(Box::into_raw)
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_rtc_engine_free(engine: *mut RtcEngine) {
    if !engine.is_null() {
        drop(Box::from_raw(engine));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_rtc_add_local_udp_candidate(
    engine: *mut RtcEngine,
    address: *const u8,
    address_len: usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let engine = engine.as_mut().ok_or("null RTC engine")?;
        let address = str::from_utf8(bytes(address, address_len)?).map_err(|e| e.to_string())?;
        let address = address.parse::<SocketAddr>().map_err(|e| e.to_string())?;
        engine.add_local_udp_candidate(address)
    }))
    .ok()
    .and_then(Result::ok)
    .map(|_| 0)
    .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_rtc_accept_offer(
    engine: *mut RtcEngine,
    offer: *const u8,
    offer_len: usize,
    answer: *mut RtcBuffer,
) -> i32 {
    if answer.is_null() {
        return -1;
    }
    (*answer).data = ptr::null_mut();
    (*answer).len = 0;
    catch_unwind(AssertUnwindSafe(|| {
        let engine = engine.as_mut().ok_or("null RTC engine")?;
        let answer_bytes = engine.accept_offer(bytes(offer, offer_len)?)?;
        let boxed = answer_bytes.into_boxed_slice();
        (*answer).len = boxed.len();
        (*answer).data = Box::into_raw(boxed) as *mut u8;
        Ok::<(), String>(())
    }))
    .ok()
    .and_then(Result::ok)
    .map(|_| 0)
    .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_rtc_create_offer(
    engine: *mut RtcEngine,
    offer: *mut RtcBuffer,
) -> i32 {
    if offer.is_null() {
        return -1;
    }
    (*offer) = empty_buffer();
    catch_unwind(AssertUnwindSafe(|| {
        let bytes = engine.as_mut().ok_or("null RTC engine")?.create_offer()?;
        (*offer) = owned_buffer(bytes);
        Ok::<(), String>(())
    }))
    .ok()
    .and_then(Result::ok)
    .map(|_| 0)
    .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_rtc_accept_answer(
    engine: *mut RtcEngine,
    answer: *const u8,
    answer_len: usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        engine
            .as_mut()
            .ok_or("null RTC engine")?
            .accept_answer(bytes(answer, answer_len)?)
    }))
    .ok()
    .and_then(Result::ok)
    .map(|_| 0)
    .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_rtc_send(
    engine: *mut RtcEngine,
    message: *const u8,
    message_len: usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        engine
            .as_mut()
            .ok_or("null RTC engine")?
            .write(bytes(message, message_len)?)
    }))
    .ok()
    .and_then(Result::ok)
    .map(|accepted| i32::from(accepted))
    .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_rtc_handle_timeout(engine: *mut RtcEngine) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        engine
            .as_mut()
            .ok_or("null RTC engine")?
            .handle_timeout(Instant::now())
    }))
    .ok()
    .and_then(Result::ok)
    .map(|_| 0)
    .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_rtc_handle_udp(
    engine: *mut RtcEngine,
    source: *const u8,
    source_len: usize,
    destination: *const u8,
    destination_len: usize,
    contents: *const u8,
    contents_len: usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let engine = engine.as_mut().ok_or("null RTC engine")?;
        let source = str::from_utf8(bytes(source, source_len)?)
            .map_err(|error| error.to_string())?
            .parse::<SocketAddr>()
            .map_err(|error| error.to_string())?;
        let destination = str::from_utf8(bytes(destination, destination_len)?)
            .map_err(|error| error.to_string())?
            .parse::<SocketAddr>()
            .map_err(|error| error.to_string())?;
        engine.handle_udp(
            Instant::now(),
            source,
            destination,
            bytes(contents, contents_len)?,
        )
    }))
    .ok()
    .and_then(Result::ok)
    .map(|_| 0)
    .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_rtc_poll(engine: *mut RtcEngine, output: *mut RtcPoll) -> i32 {
    if output.is_null() {
        return -1;
    }
    (*output) = RtcPoll {
        kind: 5,
        timeout_millis: 0,
        source: empty_buffer(),
        destination: empty_buffer(),
        payload: empty_buffer(),
        binary: 0,
    };
    catch_unwind(AssertUnwindSafe(|| {
        let event = engine.as_mut().ok_or("null RTC engine")?.poll_output()?;
        match event {
            RtcOutput::Timeout(at) => {
                (*output).kind = 0;
                (*output).timeout_millis = at
                    .saturating_duration_since(Instant::now())
                    .as_millis()
                    .try_into()
                    .unwrap_or(u64::MAX);
            }
            RtcOutput::Transmit {
                source,
                destination,
                contents,
            } => {
                (*output).kind = 1;
                (*output).source = owned_buffer(source.to_string().into_bytes());
                (*output).destination = owned_buffer(destination.to_string().into_bytes());
                (*output).payload = owned_buffer(contents);
            }
            RtcOutput::Event(Event::ChannelData(data)) => {
                (*output).kind = 2;
                (*output).payload = owned_buffer(data.data);
                (*output).binary = u32::from(data.binary);
            }
            RtcOutput::Event(Event::Connected) => (*output).kind = 3,
            RtcOutput::Event(Event::ChannelClose(_)) => (*output).kind = 4,
            RtcOutput::Event(_) => (*output).kind = 5,
        }
        Ok::<(), String>(())
    }))
    .ok()
    .and_then(Result::ok)
    .map(|_| 0)
    .unwrap_or(-1)
}

impl RtcEngine {
    pub fn new(now: Instant, label: &str, max_message_bytes: usize) -> Result<Self, String> {
        if label.is_empty()
            || label.len() > 64
            || !label
                .chars()
                .all(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.'))
        {
            return Err("RTC data-channel label must be 1..64 safe ASCII characters".into());
        }
        if !(1..=MAX_MESSAGE_BYTES).contains(&max_message_bytes) {
            return Err(format!(
                "RTC max message size must be between 1 and {MAX_MESSAGE_BYTES} bytes"
            ));
        }
        Ok(Self {
            rtc: Rtc::new(now),
            label: label.into(),
            channel: None,
            pending_offer: None,
            max_message_bytes,
            outputs: VecDeque::new(),
        })
    }

    pub fn max_message_bytes(&self) -> usize {
        self.max_message_bytes
    }

    pub fn is_connected(&self) -> bool {
        self.rtc.is_connected()
    }

    pub fn validate_message(&self, message: &[u8]) -> Result<(), String> {
        if message.is_empty() || message.len() > self.max_message_bytes {
            return Err("RTC message is empty or exceeds the configured bound".into());
        }
        Ok(())
    }

    pub fn add_local_udp_candidate(&mut self, address: SocketAddr) -> Result<(), String> {
        let candidate = Candidate::host(address, "udp").map_err(|error| error.to_string())?;
        self.rtc
            .add_local_candidate(candidate)
            .map(|_| ())
            .ok_or_else(|| "RTC local candidate was rejected".to_owned())?;
        self.drain_outputs()
    }

    pub fn accept_offer(&mut self, json: &[u8]) -> Result<Vec<u8>, String> {
        if json.is_empty() || json.len() > MAX_SDP_BYTES {
            return Err("RTC offer is empty or exceeds the configured bound".into());
        }
        let offer: SdpOffer =
            serde_json::from_slice(json).map_err(|error| format!("Invalid RTC offer: {error}"))?;
        let answer = self
            .rtc
            .sdp_api()
            .accept_offer(offer)
            .map_err(|error| format!("RTC offer rejected: {error}"))?;
        let answer = serde_json::to_vec(&answer)
            .map_err(|error| format!("Could not encode RTC answer: {error}"))?;
        self.drain_outputs()?;
        Ok(answer)
    }

    pub fn create_offer(&mut self) -> Result<Vec<u8>, String> {
        if self.pending_offer.is_some() || self.channel.is_some() {
            return Err("RTC offer has already been created".into());
        }
        let mut changes = self.rtc.sdp_api();
        let channel = changes.add_channel_with_config(ChannelConfig {
            negotiated: None,
            label: self.label.clone(),
            ordered: true,
            reliability: Reliability::Reliable,
            protocol: "hoplite.duplex/0-alpha".into(),
        });
        let (offer, pending) = changes
            .apply()
            .ok_or_else(|| "RTC offer produced no negotiation".to_owned())?;
        self.channel = Some(channel);
        self.pending_offer = Some(pending);
        let offer = serde_json::to_vec(&offer)
            .map_err(|error| format!("Could not encode RTC offer: {error}"))?;
        self.drain_outputs()?;
        Ok(offer)
    }

    pub fn accept_answer(&mut self, json: &[u8]) -> Result<(), String> {
        if json.is_empty() || json.len() > MAX_SDP_BYTES {
            return Err("RTC answer is empty or exceeds the configured bound".into());
        }
        let answer: SdpAnswer =
            serde_json::from_slice(json).map_err(|error| format!("Invalid RTC answer: {error}"))?;
        let pending = self
            .pending_offer
            .take()
            .ok_or_else(|| "RTC engine has no pending offer".to_owned())?;
        self.rtc
            .sdp_api()
            .accept_answer(pending, answer)
            .map_err(|error| format!("RTC answer rejected: {error}"))?;
        self.drain_outputs()
    }

    pub fn write(&mut self, message: &[u8]) -> Result<bool, String> {
        self.validate_message(message)?;
        let channel = self
            .channel
            .ok_or_else(|| "RTC data channel is unavailable".to_owned())?;
        let accepted = self
            .rtc
            .channel(channel)
            .ok_or_else(|| "RTC data channel is unavailable".to_owned())?
            .write(true, message)
            .map_err(|error| error.to_string())?;
        self.drain_outputs()?;
        Ok(accepted)
    }

    pub fn handle_timeout(&mut self, now: Instant) -> Result<(), String> {
        self.rtc
            .handle_input(Input::Timeout(now))
            .map_err(|error| error.to_string())?;
        self.drain_outputs()
    }

    pub fn handle_udp(
        &mut self,
        now: Instant,
        source: SocketAddr,
        destination: SocketAddr,
        contents: &[u8],
    ) -> Result<(), String> {
        let contents = contents
            .try_into()
            .map_err(|error| format!("Invalid RTC datagram: {error}"))?;
        self.rtc
            .handle_input(Input::Receive(
                now,
                Receive {
                    proto: Protocol::Udp,
                    source,
                    destination,
                    contents,
                },
            ))
            .map_err(|error| error.to_string())?;
        self.drain_outputs()
    }

    pub fn poll_output(&mut self) -> Result<RtcOutput, String> {
        self.outputs
            .pop_front()
            .ok_or_else(|| "RTC output queue is empty".to_owned())
    }

    fn drain_outputs(&mut self) -> Result<(), String> {
        loop {
            let output = match self.rtc.poll_output().map_err(|error| error.to_string())? {
                Output::Timeout(at) => RtcOutput::Timeout(at),
                Output::Transmit(transmit) => RtcOutput::Transmit {
                    source: transmit.source,
                    destination: transmit.destination,
                    contents: transmit.contents.to_vec(),
                },
                Output::Event(Event::ChannelOpen(channel, ref label)) if label == &self.label => {
                    self.channel = Some(channel);
                    RtcOutput::Event(Event::ChannelOpen(channel, label.clone()))
                }
                Output::Event(event) => RtcOutput::Event(event),
            };
            let done = matches!(output, RtcOutput::Timeout(_));
            self.outputs.push_back(output);
            if done {
                return Ok(());
            }
        }
    }

    pub fn channel_id(&self) -> Option<ChannelId> {
        self.channel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructs_a_bounded_in_band_data_channel_engine() {
        let engine = RtcEngine::new(Instant::now(), "tahto.sync", 65_536).unwrap();
        assert_eq!(engine.max_message_bytes(), 65_536);
        assert!(!engine.is_connected());
        assert!(engine.validate_message(b"signal-complete").is_ok());
        assert!(engine.validate_message(&vec![0; 65_537]).is_err());
        assert!(engine.channel_id().is_none());
        assert_eq!(ENGINE, "str0m/0.21.0");
    }

    #[test]
    fn rejects_unbounded_or_unsafe_configuration() {
        assert!(RtcEngine::new(Instant::now(), "unsafe label", 65_536).is_err());
        assert!(RtcEngine::new(Instant::now(), "safe", MAX_MESSAGE_BYTES + 1).is_err());
    }

    #[test]
    fn rejects_unbounded_or_malformed_offers_before_mutating_the_engine() {
        let mut engine = RtcEngine::new(Instant::now(), "safe", 65_536).unwrap();
        assert!(engine.accept_offer(&[]).is_err());
        assert!(engine.accept_offer(b"not-json").is_err());
        assert!(engine.accept_offer(&vec![b' '; MAX_SDP_BYTES + 1]).is_err());
    }

    #[test]
    fn accepts_an_offer_from_another_pinned_engine() {
        let now = Instant::now();
        let mut offerer = RtcEngine::new(now, "safe", 65_536).unwrap();
        let mut answerer = RtcEngine::new(now, "safe", 65_536).unwrap();
        let offer = offerer.create_offer().unwrap();
        let answer = answerer.accept_offer(&offer).unwrap();
        offerer.accept_answer(&answer).unwrap();
        assert!(offerer.channel_id().is_some());
        assert!(offerer.accept_answer(&answer).is_err());
    }

    #[test]
    fn every_mutation_is_drained_into_the_worker_queue() {
        let mut engine = RtcEngine::new(Instant::now(), "safe", 65_536).unwrap();
        engine.create_offer().unwrap();
        let mut saw_timeout = false;
        while let Ok(output) = engine.poll_output() {
            if matches!(output, RtcOutput::Timeout(_)) {
                saw_timeout = true;
            }
        }
        assert!(saw_timeout);
        assert!(engine.poll_output().is_err());
    }

    #[test]
    fn ffi_owns_an_opaque_engine_and_answer_buffer() {
        let label = b"tahto.sync";
        let engine = unsafe {
            hoplite_rtc_engine_new(label.as_ptr(), label.len(), DEFAULT_MAX_MESSAGE_BYTES)
        };
        assert!(!engine.is_null());
        let mut offerer = Rtc::new(Instant::now());
        let mut changes = offerer.sdp_api();
        changes.add_channel("tahto.sync".into());
        let (offer, _) = changes.apply().unwrap();
        let offer = serde_json::to_vec(&offer).unwrap();
        let mut answer = RtcBuffer {
            data: ptr::null_mut(),
            len: 0,
        };
        assert_eq!(
            unsafe { hoplite_rtc_accept_offer(engine, offer.as_ptr(), offer.len(), &mut answer) },
            0
        );
        assert!(!answer.data.is_null());
        assert!(answer.len > 0);
        unsafe {
            let answer = ptr::slice_from_raw_parts_mut(answer.data, answer.len);
            drop(Box::from_raw(answer));
            hoplite_rtc_engine_free(engine);
        }
        assert!(unsafe {
            hoplite_rtc_engine_new(label.as_ptr(), label.len(), MAX_MESSAGE_BYTES + 1)
        }
        .is_null());
    }
}
