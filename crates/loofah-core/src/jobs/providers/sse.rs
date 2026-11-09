use hypr_loofah_protocol::ProcessingJobFailure;

pub(super) struct Event {
    pub name: String,
    pub data: String,
}

#[derive(Default)]
pub(super) struct Decoder {
    line: Vec<u8>,
    data: String,
    name: String,
    frame_bytes: usize,
    had_line: bool,
    cr: bool,
}

impl Decoder {
    pub fn push(&mut self, byte: u8) -> Result<Option<Event>, ProcessingJobFailure> {
        if self.cr && byte == b'\n' {
            self.cr = false;
            return Ok(None);
        }
        self.cr = byte == b'\r';
        self.frame_bytes += 1;
        if self.frame_bytes > 1024 * 1024 {
            return Err(ProcessingJobFailure::ResourceLimit);
        }
        if !matches!(byte, b'\r' | b'\n') {
            self.line.push(byte);
            return Ok(None);
        }
        let line =
            std::str::from_utf8(&self.line).map_err(|_| ProcessingJobFailure::ProviderFailed)?;
        let first_line = !self.had_line;
        self.had_line = true;
        let line = if first_line {
            line.strip_prefix('\u{feff}').unwrap_or(line)
        } else {
            line
        };
        if line.is_empty() {
            self.frame_bytes = 0;
            let event = if self.data.is_empty() {
                None
            } else {
                self.data.pop();
                Some(Event {
                    name: std::mem::take(&mut self.name),
                    data: std::mem::take(&mut self.data),
                })
            };
            self.name.clear();
            self.line.clear();
            return Ok(event);
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => {
                self.data.push_str(value);
                self.data.push('\n');
            }
            "event" => {
                self.name.clear();
                self.name.push_str(value);
            }
            _ => {}
        }
        self.line.clear();
        Ok(None)
    }

    pub fn end(&self) -> Result<(), ProcessingJobFailure> {
        if !self.line.is_empty() || !self.data.is_empty() || !self.name.is_empty() {
            return Err(ProcessingJobFailure::Interrupted);
        }
        Ok(())
    }
}
