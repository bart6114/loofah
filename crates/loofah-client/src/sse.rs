use crate::{Result, invalid};

const MAX_EVENT: usize = 1024 * 1024;

#[derive(Default)]
pub(crate) struct Decoder {
    line: Vec<u8>,
    data: String,
    event: String,
    id: Option<String>,
    first: bool,
    cr: bool,
}

pub(crate) struct Message {
    pub event: String,
    pub id: Option<String>,
    pub data: String,
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            first: true,
            ..Self::default()
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Message>> {
        let mut messages = Vec::new();
        for byte in bytes {
            if self.cr {
                self.cr = false;
                if *byte == b'\n' {
                    continue;
                }
            }
            if matches!(*byte, b'\r' | b'\n') {
                self.cr = *byte == b'\r';
                let line = std::mem::take(&mut self.line);
                self.line(&line, &mut messages)?;
            } else {
                self.line.push(*byte);
                if self.line.len() > MAX_EVENT {
                    return Err(invalid("Event line exceeds the size limit"));
                }
            }
        }
        Ok(messages)
    }

    fn line(&mut self, bytes: &[u8], messages: &mut Vec<Message>) -> Result<()> {
        let mut line =
            std::str::from_utf8(bytes).map_err(|_| invalid("Event stream is not UTF-8"))?;
        if self.first {
            line = line.strip_prefix('\u{feff}').unwrap_or(line);
            self.first = false;
        }
        if line.is_empty() {
            if !self.data.is_empty() {
                self.data.pop();
                messages.push(Message {
                    event: std::mem::take(&mut self.event),
                    id: self.id.take(),
                    data: std::mem::take(&mut self.data),
                });
            }
            self.event.clear();
            self.id = None;
        } else if !line.starts_with(':') {
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "data" => {
                    if self.data.len().saturating_add(value.len() + 1) > MAX_EVENT {
                        return Err(invalid("Event exceeds the size limit"));
                    }
                    self.data.push_str(value);
                    self.data.push('\n');
                }
                "event" if value.len() <= 128 => self.event = value.into(),
                "id" if !value.contains('\0') && value.len() <= 128 => self.id = Some(value.into()),
                "id" | "event" => return Err(invalid("Invalid event header")),
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arbitrary_byte_splits_crlf_comments_bom_and_multiline_data() {
        let mut decoder = Decoder::new();
        let mut messages = Vec::new();
        for byte in "\u{feff}:keepalive\r\nevent: change\r\nid: cursor\r\ndata: café\r\ndata: next\r\n\r\ndata: incomplete".as_bytes() {
            messages.extend(decoder.push(&[*byte]).unwrap());
        }
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].event, "change");
        assert_eq!(messages[0].id.as_deref(), Some("cursor"));
        assert_eq!(messages[0].data, "café\nnext");
    }
    #[test]
    fn oversized_or_invalid_frames_are_rejected_before_dispatch() {
        assert!(Decoder::new().push(&vec![b'a'; MAX_EVENT + 1]).is_err());
        assert!(Decoder::new().push(b"data: \xff\n\n").is_err());
        assert!(Decoder::new().push(b"id: zero\0byte\n\n").is_err());
    }
}
