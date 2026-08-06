use crate::{BodyError, ResponseBody};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end_exclusive: u64,
}

impl ByteRange {
    pub fn len(self) -> u64 {
        self.end_exclusive.saturating_sub(self.start)
    }

    pub fn is_empty(self) -> bool {
        self.start >= self.end_exclusive
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RangeError {
    UnsupportedUnit,
    MultipleRanges,
    Malformed,
    Unsatisfiable { total_len: u64 },
}

impl fmt::Display for RangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedUnit => write!(formatter, "only bytes ranges are supported"),
            Self::MultipleRanges => write!(formatter, "multiple ranges are not supported"),
            Self::Malformed => write!(formatter, "malformed byte range"),
            Self::Unsatisfiable { total_len } => {
                write!(formatter, "byte range is unsatisfiable for length {total_len}")
            }
        }
    }
}

impl std::error::Error for RangeError {}

pub fn resolve_single_range(header: &str, total_len: u64) -> Result<ByteRange, RangeError> {
    let value = header
        .strip_prefix("bytes=")
        .ok_or(RangeError::UnsupportedUnit)?;
    if value.contains(',') {
        return Err(RangeError::MultipleRanges);
    }
    let (start_text, end_text) = value.split_once('-').ok_or(RangeError::Malformed)?;
    if start_text.is_empty() {
        let suffix = parse_u64(end_text)?;
        if suffix == 0 || total_len == 0 {
            return Err(RangeError::Unsatisfiable { total_len });
        }
        return Ok(ByteRange {
            start: total_len.saturating_sub(suffix),
            end_exclusive: total_len,
        });
    }

    let start = parse_u64(start_text)?;
    if start >= total_len {
        return Err(RangeError::Unsatisfiable { total_len });
    }
    let end_exclusive = if end_text.is_empty() {
        total_len
    } else {
        let inclusive_end = parse_u64(end_text)?;
        if inclusive_end < start {
            return Err(RangeError::Malformed);
        }
        inclusive_end.saturating_add(1).min(total_len)
    };
    Ok(ByteRange {
        start,
        end_exclusive,
    })
}

fn parse_u64(value: &str) -> Result<u64, RangeError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(RangeError::Malformed);
    }
    value.parse().map_err(|_| RangeError::Malformed)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponsePlan {
    pub status: u16,
    pub range: ByteRange,
    pub content_length: u64,
    pub content_range: Option<String>,
    pub accept_ranges: &'static str,
}

impl ResponsePlan {
    pub fn new(total_len: u64, range_header: Option<&str>) -> Result<Self, RangeError> {
        match range_header {
            None => Ok(Self {
                status: 200,
                range: ByteRange {
                    start: 0,
                    end_exclusive: total_len,
                },
                content_length: total_len,
                content_range: None,
                accept_ranges: "bytes",
            }),
            Some(header) => {
                let range = resolve_single_range(header, total_len)?;
                Ok(Self {
                    status: 206,
                    range,
                    content_length: range.len(),
                    content_range: Some(format!(
                        "bytes {}-{}/{}",
                        range.start,
                        range.end_exclusive - 1,
                        total_len
                    )),
                    accept_ranges: "bytes",
                })
            }
        }
    }
}

pub struct StreamResponse<B> {
    body: B,
    plan: ResponsePlan,
    cursor: u64,
}

impl<B: ResponseBody> StreamResponse<B> {
    pub fn new(body: B, range_header: Option<&str>) -> Result<Self, RangeError> {
        let plan = ResponsePlan::new(body.len(), range_header)?;
        let cursor = plan.range.start;
        Ok(Self { body, plan, cursor })
    }

    pub fn plan(&self) -> &ResponsePlan {
        &self.plan
    }

    pub fn read_next(&mut self, output: &mut [u8]) -> Result<usize, BodyError> {
        if output.is_empty() || self.cursor >= self.plan.range.end_exclusive {
            return Ok(0);
        }
        let remaining = self.plan.range.end_exclusive - self.cursor;
        let requested = output
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let read = self.body.read_at(self.cursor, &mut output[..requested])?;
        if read > requested {
            return Err(BodyError::SourceReadPastRequest {
                requested,
                returned: read,
            });
        }
        if read == 0 && requested != 0 {
            return Err(BodyError::UnexpectedEof {
                expected: self.plan.range.end_exclusive,
                observed: self.cursor,
            });
        }
        self.cursor = self.cursor.saturating_add(read as u64);
        Ok(read)
    }

    pub fn finish(&self) -> Result<(), BodyError> {
        if self.cursor == self.plan.range.end_exclusive {
            Ok(())
        } else {
            Err(BodyError::UnexpectedEof {
                expected: self.plan.range.end_exclusive,
                observed: self.cursor,
            })
        }
    }

    pub fn into_inner(self) -> B {
        self.body
    }
}
