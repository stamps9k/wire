//! WebSocket frames (RFC 6455 section 5): parsing the frames a client
//! sends and writing the frames the server sends.
//!
//! Masking and the length encoding exist only on the wire. A [`Frame`] holds
//! what a frame means, and [`Frame::parse`] and [`Frame::to_bytes`] convert
//! between that and the bytes in each direction.

/// The kind of a frame, from the low four bits of its first byte.
pub enum OpCode {
  /// A later fragment of a text or binary message.
  Continuation,
  /// A text message, or its first fragment. Must hold UTF-8.
  Text,
  /// A binary message, or its first fragment.
  Binary,
  /// Starts or answers the closing handshake.
  Close,
  /// Asks the other side for a pong with the same payload.
  Ping,
  /// Answers a ping, or serves as a one-way heartbeat.
  Pong,
}

impl OpCode {
  /// The four opcode bits, as they appear in the low half of a frame's
  /// first byte.
  pub fn bits(&self) -> u8 {
    match self {
      OpCode::Continuation => 0x00,
      OpCode::Text => 0x01,
      OpCode::Binary => 0x02,
      OpCode::Close => 0x08,
      OpCode::Ping => 0x09,
      OpCode::Pong => 0xA,
    }
  }
}

/// One WebSocket frame, without the details of how it is sent.
pub struct Frame {
  /// Whether this is the last frame of its message.
  pub fin: bool,
  /// What kind of frame this is.
  pub opcode: OpCode,
  /// The payload, never masked.
  pub payload: Vec<u8>,
}

/// Why a frame from a client was refused. Whatever the reason, the
/// connection is closed, with the status code from
/// [`close_code`](Self::close_code) and the text from
/// [`reason`](Self::reason).
pub enum FrameError {
  /// The frame breaks a rule of RFC 6455; close with 1002 (protocol error).
  /// The reason names the rule.
  Protocol(&'static str),
  /// The payload is over the size the caller accepts; close with 1009
  /// (message too big).
  TooLarge,
  /// Text that must be UTF-8 is not; close with 1007 (invalid frame
  /// payload data).
  InvalidUtf8,
}

/// Bytes received from a client, from which whole frames are taken.
#[derive(Default)]
pub struct FrameBuffer {
  bytes: Vec<u8>,
}

/// Bytes received from a client, from which whole frames are taken one at a
/// time.
///
/// Reads from a socket do not line up with frames: one read can hold part
/// of a frame, a whole frame, or several. Bytes are added as they arrive
/// with [`extend`](Self::extend), and [`next_frame`](Self::next_frame)
/// hands out each complete frame once, keeping whatever follows it for the
/// next call.
impl FrameBuffer {
  /// Adds bytes read from the client to the end of the buffer.
  pub fn extend(&mut self, bytes: &[u8]) {
    self.bytes.extend_from_slice(bytes);
  }

  /// Takes the next complete frame from the front of the buffer, checked
  /// and unmasked by [`Frame::parse`] with a payload cap of `max_payload`.
  ///
  /// Returns `Ok(None)` while no complete frame is buffered. A frame that
  /// is returned has already been removed from the buffer, so the next call
  /// starts at the frame after it, and calling this until it returns
  /// `Ok(None)` takes every frame that has fully arrived.
  ///
  /// # Errors
  ///
  /// Returns the [`FrameError`] from [`Frame::parse`] if the frame at the
  /// front breaks the protocol or is over `max_payload`. The buffer is left
  /// as it was, since the connection is closed after an error.
  pub fn next_frame(
    &mut self,
    max_payload: usize,
  ) -> Result<Option<Frame>, FrameError> {
    let Some((frame, used)) = Frame::parse(&self.bytes, max_payload)? else {
      return Ok(None);
    };
    self.bytes.drain(..used);
    Ok(Some(frame))
  }
}

impl FrameError {
  /// The WebSocket status code to close the connection with.
  pub fn close_code(&self) -> u16 {
    match self {
      Self::Protocol(_) => 1002,
      Self::TooLarge => 1009,
      Self::InvalidUtf8 => 1007,
    }
  }

  /// The reason to send with the close, naming what was wrong.
  ///
  /// A protocol error gives the rule it broke; the other variants have a
  /// fixed text. Every reason is well under the 123 bytes a close frame
  /// leaves for it.
  pub fn reason(&self) -> &'static str {
    match self {
      Self::Protocol(reason) => reason,
      Self::TooLarge => "frame too large",
      Self::InvalidUtf8 => "invalid utf-8",
    }
  }
}

impl Frame {
  /// Writes the frame as a server sends it.
  ///
  /// The first byte holds FIN and the opcode. The length follows in the
  /// shortest form RFC 6455 allows: one byte up to 125, then `126` and a
  /// big-endian `u16` up to 65535, then `127` and a big-endian `u64`. Server
  /// frames are never masked, so the mask bit is clear and the payload is
  /// copied unchanged.
  pub fn to_bytes(&self) -> Vec<u8> {
    let payload = &self.payload;
    let len = u64::try_from(payload.len()).unwrap_or(u64::MAX);
    let mut out = Vec::with_capacity(payload.len().saturating_add(10));
    let fin_bit = if self.fin { 0x80 } else { 0x00 };
    let opcode_bits = self.opcode.bits();

    out.push(fin_bit | opcode_bits);

    match (u8::try_from(len), u16::try_from(len)) {
      (Ok(short), _) if short < 126 => out.push(short),
      (_, Ok(medium)) => {
        out.push(126);
        out.extend_from_slice(&medium.to_be_bytes());
      }
      _ => {
        out.push(127);
        out.extend_from_slice(&len.to_be_bytes());
      }
    }
    out.extend_from_slice(payload);
    out
  }

  /// Builds a close frame carrying `code` and `reason`.
  ///
  /// The payload is the status code as a big-endian `u16` followed by the
  /// reason's UTF-8 bytes, with nothing between them; an empty `reason`
  /// leaves just the code. RFC 6455 limits a close payload to 125 bytes, so
  /// the caller must keep `reason` to 123 bytes or fewer, and must not send
  /// a code reserved for local use (1005, 1006, 1015).
  pub fn close(code: u16, reason: &str) -> Frame {
    let mut payload = code.to_be_bytes().to_vec();
    payload.extend_from_slice(reason.as_bytes());
    Frame {
      fin: true,
      opcode: OpCode::Close,
      payload,
    }
  }

  /// Reads and checks the status code of a close frame sent by a client,
  /// from its `payload`.
  ///
  /// An empty payload carries no code and gives `Ok(None)`. Otherwise the
  /// first two bytes are the code, as a big-endian `u16`, and the rest is a
  /// reason, which must be UTF-8 but is otherwise ignored. The code must be
  /// one RFC 6455 allows on the wire: 1000 to 1003, 1007 to 1014, or 3000 to
  /// 4999 for registered and application codes. The others are either
  /// reserved (below 1000, 1004, and 1016 to 2999), only for reporting
  /// locally (1005, 1006 and 1015), or outside the range (5000 and above).
  ///
  /// # Errors
  ///
  /// Returns [`FrameError::Protocol`] if the payload is a single byte or the
  /// code is not allowed, and [`FrameError::InvalidUtf8`] if the reason is
  /// not UTF-8. The reason is checked first, so a payload with both problems
  /// gives [`FrameError::InvalidUtf8`].
  pub fn close_code(payload: &[u8]) -> Result<Option<u16>, FrameError> {
    if payload.is_empty() {
      return Ok(None);
    }

    let Some(code_bytes) = payload.first_chunk::<2>() else {
      return Err(FrameError::Protocol("close payload has one byte"));
    };
    let code = u16::from_be_bytes(*code_bytes);

    if std::str::from_utf8(payload.get(2..).unwrap_or_default()).is_err() {
      return Err(FrameError::InvalidUtf8);
    }

    match code {
      0..=999 | 1004..=1006 | 1015..=2999 | 5000..=u16::MAX => {
        Err(FrameError::Protocol("invalid close code"))
      }
      1000..=1003 | 1007..=1014 | 3000..=4999 => Ok(Some(code)),
    }
  }

  /// Builds a single, complete text frame holding `payload`.
  ///
  /// The caller must make sure the payload is valid UTF-8, because a text
  /// frame must hold UTF-8.
  pub fn text(payload: &[u8]) -> Frame {
    Frame {
      fin: true,
      opcode: OpCode::Text,
      payload: payload.to_vec(),
    }
  }

  /// Parses one frame sent by a client from the start of `buf`.
  ///
  /// Checks the frame against RFC 6455 as it goes: reserved bits clear, a
  /// known opcode, the mask bit set, control frames unfragmented and at most
  /// 125 bytes, and the top bit of an eight-byte length clear. A payload
  /// longer than `max_payload` is refused as soon as its length is known,
  /// without waiting for it to arrive. The payload is returned unmasked.
  ///
  /// Returns `Ok(None)` while the frame has not fully arrived, and
  /// `Ok(Some((frame, used)))` once it has, where `used` is the number of
  /// bytes the frame took up. Anything after them is left for the next call.
  ///
  /// # Errors
  ///
  /// Returns [`FrameError::Protocol`] if the frame breaks a rule above, and
  /// [`FrameError::TooLarge`] if its payload is over `max_payload`.
  pub fn parse(
    buf: &[u8],
    max_payload: usize,
  ) -> Result<Option<(Frame, usize)>, FrameError> {
    let mut header_size: usize = 6; // first two bytes + the 4-byte masking key

    let Some(first) = buf.first() else {
      return Ok(None);
    };

    let fin = first & 0x80 != 0;
    if first & 0x70 != 0 {
      return Err(FrameError::Protocol("reserved bits set"));
    }
    let opcode = match first & 0x0F {
      0x0 => OpCode::Continuation,
      0x1 => OpCode::Text,
      0x2 => OpCode::Binary,
      0x8 => OpCode::Close,
      0x9 => OpCode::Ping,
      0xA => OpCode::Pong,
      _ => return Err(FrameError::Protocol("unknown opcode")),
    };
    let is_control =
      matches!(opcode, OpCode::Close | OpCode::Ping | OpCode::Pong);

    if is_control && !fin {
      return Err(FrameError::Protocol("fragmented control frame"));
    }

    let Some(second) = buf.get(1).copied() else {
      return Ok(None);
    };
    if second & 0x80 == 0 {
      return Err(FrameError::Protocol("unmasked client frame"));
    }

    let length = match second & 0x7F {
      126 => {
        header_size = header_size
          .checked_add(2)
          .ok_or(FrameError::Protocol("header size overflowed"))?;
        let Some(length_bytes) =
          buf.get(2..).and_then(<[u8]>::first_chunk::<2>)
        else {
          return Ok(None);
        };
        u64::from(u16::from_be_bytes(*length_bytes))
      }
      127 => {
        header_size = header_size
          .checked_add(8)
          .ok_or(FrameError::Protocol("header size overflowed"))?;
        let Some(length_bytes) =
          buf.get(2..).and_then(<[u8]>::first_chunk::<8>)
        else {
          return Ok(None);
        };

        if length_bytes[0].leading_zeros() == 0 {
          return Err(FrameError::Protocol("length has the top bit set"));
        }

        u64::from_be_bytes(*length_bytes)
      }
      short => u64::from(short),
    };

    let Ok(length) = usize::try_from(length) else {
      return Err(FrameError::TooLarge);
    };

    if is_control && length > 125 {
      return Err(FrameError::Protocol("control frame too long"));
    }

    if length > max_payload {
      return Err(FrameError::TooLarge);
    }

    let key_start = header_size
      .checked_sub(4)
      .ok_or(FrameError::Protocol("header size underflow"))?;
    let Some(key) = buf.get(key_start..header_size) else {
      return Ok(None);
    };

    let start = header_size;
    let end = header_size
      .checked_add(length)
      .ok_or(FrameError::Protocol("frame size overflowed"))?;
    // The payload follows the key
    let Some(slice) = buf.get(start..end) else {
      return Ok(None);
    };

    let decoded_payload: Vec<u8> = slice
      .iter()
      .zip(key.iter().cycle())
      .map(|(byte, mask)| byte ^ mask)
      .collect();

    Ok(Some((
      Frame {
        fin,
        opcode,
        payload: decoded_payload,
      },
      end,
    )))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  type TestResult = Result<(), &'static str>;

  /// The masking key from the RFC 6455 section 5.7 examples.
  const KEY: [u8; 4] = [0x37, 0xfa, 0x21, 0x3d];

  /// The payload cap used by most tests.
  const MAX: usize = 1024;

  /// Builds a frame as a client sends it: `first` as the first byte, the
  /// length in its shortest form with the mask bit set, [`KEY`], then
  /// `payload` masked with it.
  fn client_frame(first: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![first];
    match (u8::try_from(payload.len()), u16::try_from(payload.len())) {
      (Ok(short), _) if short < 126 => out.push(0x80 | short),
      (_, Ok(medium)) => {
        out.push(0xfe); // mask bit + 126
        out.extend_from_slice(&medium.to_be_bytes());
      }
      _ => {
        let long = u64::try_from(payload.len()).unwrap_or(u64::MAX);
        out.push(0xff); // mask bit + 127
        out.extend_from_slice(&long.to_be_bytes());
      }
    }
    out.extend_from_slice(&KEY);
    out.extend(
      payload
        .iter()
        .zip(KEY.iter().cycle())
        .map(|(byte, mask)| byte ^ mask),
    );
    out
  }

  /// Parses `buf` with the default cap and returns the frame and the bytes
  /// used, or `None` if it was not parsed as a whole frame.
  fn complete(buf: &[u8]) -> Option<(Frame, usize)> {
    Frame::parse(buf, MAX).ok().flatten()
  }

  /// Whether `buf` is reported as not all here yet.
  fn incomplete(buf: &[u8]) -> bool {
    matches!(Frame::parse(buf, MAX), Ok(None))
  }

  /// Whether `buf` is refused as a protocol error.
  fn protocol_error(buf: &[u8]) -> bool {
    matches!(Frame::parse(buf, MAX), Err(FrameError::Protocol(_)))
  }

  /// Whether `buf` is refused as larger than `max_payload`.
  fn too_large(buf: &[u8], max_payload: usize) -> bool {
    matches!(Frame::parse(buf, max_payload), Err(FrameError::TooLarge))
  }

  // ---- parse: whole frames ----

  #[test]
  fn parses_the_rfc_masked_hello_example() -> TestResult {
    // RFC 6455 section 5.7: a single-frame masked text message.
    let bytes = [
      0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58,
    ];
    let (frame, used) = complete(&bytes).ok_or("not a whole frame")?;
    assert!(frame.fin);
    assert!(matches!(frame.opcode, OpCode::Text));
    assert_eq!(frame.payload, b"Hello");
    assert_eq!(used, 11);
    Ok(())
  }

  #[test]
  fn parses_a_close_frame_with_a_status_code() -> TestResult {
    // 1000 (normal closure) as a big-endian u16.
    let bytes = client_frame(0x88, &[0x03, 0xe8]);
    let (frame, used) = complete(&bytes).ok_or("not a whole frame")?;
    assert!(frame.fin);
    assert!(matches!(frame.opcode, OpCode::Close));
    assert_eq!(frame.payload, [0x03, 0xe8]);
    assert_eq!(used, 8);
    Ok(())
  }

  #[test]
  fn parses_an_empty_close_frame() -> TestResult {
    let bytes = client_frame(0x88, b"");
    let (frame, used) = complete(&bytes).ok_or("not a whole frame")?;
    assert!(matches!(frame.opcode, OpCode::Close));
    assert!(frame.payload.is_empty());
    assert_eq!(used, 6);
    Ok(())
  }

  #[test]
  fn parses_a_ping_frame() -> TestResult {
    let (frame, _) =
      complete(&client_frame(0x89, b"abc")).ok_or("not parsed")?;
    assert!(matches!(frame.opcode, OpCode::Ping));
    assert_eq!(frame.payload, b"abc");
    Ok(())
  }

  #[test]
  fn parses_a_pong_frame() -> TestResult {
    let (frame, _) =
      complete(&client_frame(0x8a, b"abc")).ok_or("not parsed")?;
    assert!(matches!(frame.opcode, OpCode::Pong));
    assert_eq!(frame.payload, b"abc");
    Ok(())
  }

  #[test]
  fn parses_a_binary_frame() -> TestResult {
    let (frame, _) =
      complete(&client_frame(0x82, &[0x00, 0xff, 0x10])).ok_or("not parsed")?;
    assert!(matches!(frame.opcode, OpCode::Binary));
    assert_eq!(frame.payload, [0x00, 0xff, 0x10]);
    Ok(())
  }

  #[test]
  fn reports_fin_and_opcode_of_each_fragment() -> TestResult {
    // RFC 6455 section 5.4: a text message sent as three fragments.
    let (first, _) =
      complete(&client_frame(0x01, b"Hel")).ok_or("first not parsed")?;
    assert!(!first.fin);
    assert!(matches!(first.opcode, OpCode::Text));
    assert_eq!(first.payload, b"Hel");

    let (middle, _) =
      complete(&client_frame(0x00, b"lo")).ok_or("middle not parsed")?;
    assert!(!middle.fin);
    assert!(matches!(middle.opcode, OpCode::Continuation));

    let (last, _) =
      complete(&client_frame(0x80, b"!")).ok_or("last not parsed")?;
    assert!(last.fin);
    assert!(matches!(last.opcode, OpCode::Continuation));
    Ok(())
  }

  #[test]
  fn does_not_check_text_payloads_for_utf8() -> TestResult {
    // UTF-8 is a rule about whole messages, so it is left to the caller.
    let (frame, _) =
      complete(&client_frame(0x81, &[0xff, 0xfe])).ok_or("not parsed")?;
    assert_eq!(frame.payload, [0xff, 0xfe]);
    Ok(())
  }

  #[test]
  fn reads_a_two_byte_length() -> TestResult {
    let payload = [0xab; 200];
    let bytes = client_frame(0x82, &payload);
    assert!(bytes.starts_with(&[0x82, 0xfe, 0x00, 0xc8]));
    let (frame, used) = complete(&bytes).ok_or("not a whole frame")?;
    assert_eq!(frame.payload, payload);
    assert_eq!(used, 208);
    Ok(())
  }

  #[test]
  fn reads_an_eight_byte_length() -> TestResult {
    let payload = vec![0x07; 65_536];
    let bytes = client_frame(0x82, &payload);
    assert!(bytes.starts_with(&[0x82, 0xff, 0, 0, 0, 0, 0, 1, 0, 0]));
    let (frame, used) = Frame::parse(&bytes, 70_000)
      .ok()
      .flatten()
      .ok_or("not a whole frame")?;
    assert_eq!(frame.payload, payload);
    assert_eq!(used, 65_550);
    Ok(())
  }

  #[test]
  fn accepts_a_payload_exactly_at_the_cap() -> TestResult {
    let bytes = client_frame(0x82, &[0x00; MAX]);
    let (frame, used) = complete(&bytes).ok_or("not a whole frame")?;
    assert_eq!(frame.payload.len(), MAX);
    assert_eq!(used, 1032);
    Ok(())
  }

  #[test]
  fn accepts_a_control_frame_of_125_bytes() -> TestResult {
    let (frame, _) =
      complete(&client_frame(0x89, &[0x00; 125])).ok_or("not parsed")?;
    assert_eq!(frame.payload.len(), 125);
    Ok(())
  }

  // ---- parse: buffers that are not exactly one frame ----

  #[test]
  fn waits_on_an_empty_buffer() {
    assert!(incomplete(b""));
  }

  #[test]
  fn waits_while_a_short_frame_is_cut_anywhere() -> TestResult {
    let bytes = client_frame(0x81, b"Hello");
    for cut in 0..bytes.len() {
      let prefix = bytes.get(..cut).ok_or("cut past the end")?;
      assert!(incomplete(prefix), "cut at {cut}");
    }
    Ok(())
  }

  #[test]
  fn waits_while_a_two_byte_length_frame_is_cut_anywhere() -> TestResult {
    let bytes = client_frame(0x82, &[0xab; 200]);
    for cut in 0..bytes.len() {
      let prefix = bytes.get(..cut).ok_or("cut past the end")?;
      assert!(incomplete(prefix), "cut at {cut}");
    }
    Ok(())
  }

  #[test]
  fn waits_while_an_eight_byte_length_frame_is_cut() -> TestResult {
    let bytes = client_frame(0x82, &vec![0x07; 65_536]);
    // Every cut inside the 14-byte header, then one byte short of the end.
    let cuts = (0..14).chain([bytes.len().saturating_sub(1)]);
    for cut in cuts {
      let prefix = bytes.get(..cut).ok_or("cut past the end")?;
      assert!(
        matches!(Frame::parse(prefix, 70_000), Ok(None)),
        "cut at {cut}"
      );
    }
    Ok(())
  }

  #[test]
  fn uses_only_the_first_of_two_frames() -> TestResult {
    let mut bytes = client_frame(0x89, b"a");
    bytes.extend(client_frame(0x88, &[0x03, 0xe8]));

    let (first, used) = complete(&bytes).ok_or("first not parsed")?;
    assert!(matches!(first.opcode, OpCode::Ping));
    assert_eq!(used, 7);

    let rest = bytes.get(used..).ok_or("used past the end")?;
    let (second, used) = complete(rest).ok_or("second not parsed")?;
    assert!(matches!(second.opcode, OpCode::Close));
    assert_eq!(second.payload, [0x03, 0xe8]);
    assert_eq!(used, 8);
    Ok(())
  }

  #[test]
  fn ignores_the_start_of_a_following_frame() -> TestResult {
    let mut bytes = client_frame(0x89, b"");
    bytes.push(0x88);
    let (_, used) = complete(&bytes).ok_or("not a whole frame")?;
    assert_eq!(used, 6);
    Ok(())
  }

  // ---- parse: protocol errors ----

  #[test]
  fn refuses_an_unmasked_frame() {
    // The RFC "Hello" example as a server sends it.
    let bytes = [0x81, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f];
    assert!(protocol_error(&bytes));
  }

  #[test]
  fn refuses_an_unmasked_frame_from_its_first_two_bytes() {
    assert!(protocol_error(&[0x81, 0x05]));
  }

  #[test]
  fn refuses_reserved_bits() {
    for first in [0xc1, 0xa1, 0x91, 0xf1] {
      assert!(protocol_error(&client_frame(first, b"hi")), "{first:#04x}");
    }
  }

  #[test]
  fn refuses_reserved_opcodes() {
    for opcode in [0x3, 0x4, 0x5, 0x6, 0x7, 0xb, 0xc, 0xd, 0xe, 0xf] {
      let bytes = client_frame(0x80 | opcode, b"");
      assert!(protocol_error(&bytes), "{opcode:#x}");
    }
  }

  #[test]
  fn refuses_a_fragmented_control_frame() {
    for first in [0x08, 0x09, 0x0a] {
      assert!(protocol_error(&client_frame(first, b"")), "{first:#04x}");
    }
  }

  #[test]
  fn refuses_a_control_frame_over_125_bytes() {
    assert!(protocol_error(&client_frame(0x88, &[0x00; 126])));
  }

  #[test]
  fn refuses_a_long_control_frame_from_its_header() {
    // A ping announcing 126 bytes, before any key or payload arrives.
    assert!(protocol_error(&[0x89, 0xfe, 0x00, 0x7e]));
  }

  #[test]
  fn refuses_an_eight_byte_length_with_the_top_bit_set() {
    let bytes = [0x82, 0xff, 0x80, 0, 0, 0, 0, 0, 0, 0];
    assert!(protocol_error(&bytes));
  }

  // ---- parse: size cap ----

  #[test]
  fn refuses_a_payload_over_the_cap() {
    assert!(too_large(&client_frame(0x82, &[0x00; MAX + 1]), MAX));
  }

  #[test]
  fn refuses_a_payload_over_the_cap_from_its_header() {
    // 1025 bytes announced, nothing more sent: no need to wait for them.
    assert!(too_large(&[0x82, 0xfe, 0x04, 0x01], MAX));
  }

  #[test]
  fn refuses_a_huge_length_without_waiting_for_it() {
    // The largest length the RFC allows, about 9.2 exabytes.
    let bytes = [0x82, 0xff, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    assert!(too_large(&bytes, MAX));
  }

  #[test]
  fn applies_the_cap_it_is_given() {
    let bytes = client_frame(0x82, &[0x00; 10]);
    assert!(too_large(&bytes, 9));
    assert!(matches!(Frame::parse(&bytes, 10), Ok(Some(_))));
  }

  // ---- to_bytes ----

  #[test]
  fn text_frame_of_an_empty_payload_is_two_bytes() {
    assert_eq!(Frame::text(b"").to_bytes(), [0x81, 0x00]);
  }

  #[test]
  fn text_frame_matches_the_rfc_hello_example() {
    // RFC 6455 section 5.7: a single-frame unmasked text message.
    assert_eq!(
      Frame::text(b"Hello").to_bytes(),
      [0x81, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f]
    );
  }

  #[test]
  fn text_frame_uses_one_length_byte_up_to_125() {
    let payload = vec![b'a'; 125];
    let frame = Frame::text(&payload).to_bytes();
    assert!(frame.starts_with(&[0x81, 125]));
    assert_eq!(frame.len(), 127);
    assert_eq!(frame.get(2..), Some(payload.as_slice()));
  }

  #[test]
  fn text_frame_switches_to_two_length_bytes_at_126() {
    let payload = vec![b'a'; 126];
    let frame = Frame::text(&payload).to_bytes();
    assert!(frame.starts_with(&[0x81, 126, 0x00, 0x7e]));
    assert_eq!(frame.len(), 130);
    assert_eq!(frame.get(4..), Some(payload.as_slice()));
  }

  #[test]
  fn text_frame_writes_the_two_byte_length_big_endian() {
    // RFC 6455 section 5.7 gives 256 bytes as 0x7e 0x0100.
    let frame = Frame::text(&vec![b'a'; 256]).to_bytes();
    assert!(frame.starts_with(&[0x81, 126, 0x01, 0x00]));
    assert_eq!(frame.len(), 260);
  }

  #[test]
  fn text_frame_uses_two_length_bytes_up_to_65535() {
    let payload = vec![b'a'; 65_535];
    let frame = Frame::text(&payload).to_bytes();
    assert!(frame.starts_with(&[0x81, 126, 0xff, 0xff]));
    assert_eq!(frame.len(), 65_539);
    assert_eq!(frame.get(4..), Some(payload.as_slice()));
  }

  #[test]
  fn text_frame_switches_to_eight_length_bytes_at_65536() {
    // RFC 6455 section 5.7 gives 64 KiB as 0x7f 0x0000000000010000.
    let payload = vec![b'a'; 65_536];
    let frame = Frame::text(&payload).to_bytes();
    assert!(frame.starts_with(&[0x81, 127, 0, 0, 0, 0, 0, 1, 0, 0]));
    assert_eq!(frame.len(), 65_546);
    assert_eq!(frame.get(10..), Some(payload.as_slice()));
  }

  #[test]
  fn text_frame_never_sets_the_mask_bit() {
    // A server must not mask the frames it sends.
    for length in [0, 1, 125, 126, 127, 128, 255, 256, 65_535, 65_536] {
      let frame = Frame::text(&vec![0xff; length]).to_bytes();
      assert_eq!(frame.get(1).map(|byte| byte & 0x80), Some(0), "{length}");
    }
  }

  #[test]
  fn text_frame_always_starts_with_fin_and_the_text_opcode() {
    for length in [0, 1, 125, 126, 65_535, 65_536] {
      let frame = Frame::text(&vec![b'a'; length]).to_bytes();
      assert_eq!(frame.first(), Some(&0x81), "{length}");
    }
  }

  #[test]
  fn text_frame_copies_the_payload_unchanged() {
    // Bytes that look like frame headers must not be touched.
    let payload = [0x81, 0x7e, 0x7f, 0x00, 0xff, b'\r', b'\n'];
    let frame = Frame::text(&payload).to_bytes();
    assert_eq!(frame.get(..2), Some([0x81, 0x07].as_slice()));
    assert_eq!(frame.get(2..), Some(payload.as_slice()));
  }

  #[test]
  fn text_frame_counts_the_length_in_bytes() {
    // Two characters, five bytes in UTF-8.
    let frame = Frame::text("é€".as_bytes()).to_bytes();
    assert_eq!(frame.get(1), Some(&5));
    assert_eq!(frame.len(), 7);
  }

  #[test]
  fn to_bytes_writes_a_pong() {
    let frame = Frame {
      fin: true,
      opcode: OpCode::Pong,
      payload: vec![0x01, 0x02],
    };
    assert_eq!(frame.to_bytes(), [0x8a, 0x02, 0x01, 0x02]);
  }

  #[test]
  fn to_bytes_writes_a_close_with_a_status_code() {
    let frame = Frame {
      fin: true,
      opcode: OpCode::Close,
      payload: vec![0x03, 0xe8],
    };
    assert_eq!(frame.to_bytes(), [0x88, 0x02, 0x03, 0xe8]);
  }

  #[test]
  fn to_bytes_leaves_fin_clear_on_a_fragment() {
    let frame = Frame {
      fin: false,
      opcode: OpCode::Text,
      payload: b"Hel".to_vec(),
    };
    assert_eq!(frame.to_bytes().first(), Some(&0x01));
  }

  // ---- close_code ----

  #[test]
  fn protocol_errors_close_with_1002() {
    assert_eq!(FrameError::Protocol("any rule").close_code(), 1002);
  }

  #[test]
  fn oversized_payloads_close_with_1009() {
    assert_eq!(FrameError::TooLarge.close_code(), 1009);
  }

  #[test]
  fn invalid_utf8_closes_with_1007() {
    assert_eq!(FrameError::InvalidUtf8.close_code(), 1007);
  }

  // ---- reason ----

  #[test]
  fn a_protocol_error_gives_the_rule_it_broke() {
    assert_eq!(FrameError::Protocol("any rule").reason(), "any rule");
  }

  #[test]
  fn the_other_errors_give_a_fixed_reason() {
    assert_eq!(FrameError::TooLarge.reason(), "frame too large");
    assert_eq!(FrameError::InvalidUtf8.reason(), "invalid utf-8");
  }

  // ---- Frame::close_code ----

  /// A close payload: `code` as a big-endian `u16`, then `reason`.
  fn close_payload(code: u16, reason: &[u8]) -> Vec<u8> {
    let mut payload = code.to_be_bytes().to_vec();
    payload.extend_from_slice(reason);
    payload
  }

  /// Whether `payload` is refused as a protocol error.
  fn bad_close(payload: &[u8]) -> bool {
    matches!(Frame::close_code(payload), Err(FrameError::Protocol(_)))
  }

  #[test]
  fn a_close_with_no_payload_has_no_code() {
    assert!(matches!(Frame::close_code(&[]), Ok(None)));
  }

  #[test]
  fn a_close_with_one_byte_is_a_protocol_error() {
    assert!(bad_close(&[0x03]));
  }

  #[test]
  fn reads_the_code_as_big_endian() {
    assert!(matches!(Frame::close_code(&[0x03, 0xe8]), Ok(Some(1000))));
    // The same bytes the other way round are 59395, which is not allowed.
    assert!(bad_close(&[0xe8, 0x03]));
  }

  #[test]
  fn accepts_every_edge_of_the_allowed_ranges() {
    for code in [1000, 1003, 1007, 1014, 3000, 4000, 4999] {
      assert!(
        matches!(
          Frame::close_code(&close_payload(code, b"")),
          Ok(Some(c)) if c == code
        ),
        "{code} was refused"
      );
    }
  }

  #[test]
  fn refuses_every_edge_of_the_forbidden_ranges() {
    for code in [0, 999, 1004, 1005, 1006, 1015, 1016, 2999, 5000, u16::MAX] {
      assert!(bad_close(&close_payload(code, b"")), "{code} was accepted");
    }
  }

  #[test]
  fn accepts_a_utf8_reason_and_ignores_it() {
    let payload = close_payload(1001, "タブを閉じた".as_bytes());
    assert!(matches!(Frame::close_code(&payload), Ok(Some(1001))));
  }

  #[test]
  fn refuses_a_reason_that_is_not_utf8() {
    let payload = close_payload(1000, b"bad \xff byte");
    assert!(matches!(
      Frame::close_code(&payload),
      Err(FrameError::InvalidUtf8)
    ));
  }

  #[test]
  fn a_bad_reason_wins_over_a_bad_code() {
    let payload = close_payload(1005, b"\xff");
    assert!(matches!(
      Frame::close_code(&payload),
      Err(FrameError::InvalidUtf8)
    ));
  }

  // ---- FrameBuffer ----

  /// Takes the next frame from `buffer` with the default cap, or `None` if
  /// there is no whole frame or an error.
  fn next(buffer: &mut FrameBuffer) -> Option<Frame> {
    buffer.next_frame(MAX).ok().flatten()
  }

  #[test]
  fn an_empty_buffer_holds_no_frame() {
    let mut buffer = FrameBuffer::default();
    assert!(matches!(buffer.next_frame(MAX), Ok(None)));
  }

  #[test]
  fn a_frame_is_taken_once() -> TestResult {
    let mut buffer = FrameBuffer::default();
    buffer.extend(&client_frame(0x89, b"once"));

    let frame = next(&mut buffer).ok_or("no frame")?;
    assert!(matches!(frame.opcode, OpCode::Ping));
    assert_eq!(frame.payload, b"once");
    // Taking it removed it: the buffer is empty again.
    assert!(matches!(buffer.next_frame(MAX), Ok(None)));
    Ok(())
  }

  #[test]
  fn frames_are_taken_in_order() -> TestResult {
    let mut buffer = FrameBuffer::default();
    let mut bytes = client_frame(0x89, b"one");
    bytes.extend(client_frame(0x89, b"two"));
    bytes.extend(client_frame(0x88, &[0x03, 0xe8]));
    buffer.extend(&bytes);

    assert_eq!(next(&mut buffer).ok_or("no first")?.payload, b"one");
    assert_eq!(next(&mut buffer).ok_or("no second")?.payload, b"two");
    let third = next(&mut buffer).ok_or("no third")?;
    assert!(matches!(third.opcode, OpCode::Close));
    assert!(matches!(buffer.next_frame(MAX), Ok(None)));
    Ok(())
  }

  #[test]
  fn a_frame_split_across_extends_is_taken_once_complete() -> TestResult {
    let bytes = client_frame(0x89, b"split");
    let (front, back) = bytes.split_at_checked(4).ok_or("frame too short")?;
    let mut buffer = FrameBuffer::default();

    buffer.extend(front);
    assert!(matches!(buffer.next_frame(MAX), Ok(None)));
    buffer.extend(back);
    assert_eq!(next(&mut buffer).ok_or("no frame")?.payload, b"split");
    Ok(())
  }

  #[test]
  fn keeps_the_start_of_the_next_frame() -> TestResult {
    let second = client_frame(0x89, b"two");
    let (front, back) = second.split_at_checked(3).ok_or("frame too short")?;
    let mut bytes = client_frame(0x89, b"one");
    bytes.extend_from_slice(front);
    let mut buffer = FrameBuffer::default();

    buffer.extend(&bytes);
    assert_eq!(next(&mut buffer).ok_or("no first")?.payload, b"one");
    assert!(matches!(buffer.next_frame(MAX), Ok(None)));
    buffer.extend(back);
    assert_eq!(next(&mut buffer).ok_or("no second")?.payload, b"two");
    Ok(())
  }

  #[test]
  fn always_moves_on_through_many_small_frames() -> TestResult {
    // 500 frames, each one byte different, so taking the same frame twice
    // or skipping one shows up as a wrong payload.
    let mut bytes = Vec::new();
    for count in 0..500_u16 {
      bytes.extend(client_frame(0x89, &count.to_be_bytes()));
    }
    let mut buffer = FrameBuffer::default();
    buffer.extend(&bytes);

    for count in 0..500_u16 {
      let frame = next(&mut buffer).ok_or("ran out of frames")?;
      assert_eq!(frame.payload, count.to_be_bytes(), "frame {count}");
    }
    assert!(matches!(buffer.next_frame(MAX), Ok(None)));
    Ok(())
  }

  #[test]
  fn passes_on_errors_from_parse() {
    let mut buffer = FrameBuffer::default();
    buffer.extend(&[0x81, 0x05]);
    assert!(matches!(
      buffer.next_frame(MAX),
      Err(FrameError::Protocol(_))
    ));

    let mut buffer = FrameBuffer::default();
    buffer.extend(&[0x82, 0xfe, 0x04, 0x01]);
    assert!(matches!(buffer.next_frame(MAX), Err(FrameError::TooLarge)));
  }
}
