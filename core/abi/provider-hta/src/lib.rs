#![forbid(unsafe_code)]

//! Dependency-free exact-span reader for canonical `HTA1` provider frames.
//!
//! The reader validates a complete frame and records the byte span of every
//! nested value. Generic native providers can inspect closed request fields
//! while persisting opaque application values byte-for-byte.

use std::fmt;
use std::ops::Range;
use std::str;

const MAGIC: &[u8; 4] = b"HTA1";

const NIL: u8 = 0;
const FALSE: u8 = 1;
const TRUE: u8 = 2;
const I64: u8 = 3;
const STRING: u8 = 4;
const BYTES: u8 = 5;
const KEYWORD: u8 = 6;
const SYMBOL: u8 = 7;
const LIST: u8 = 8;
const VECTOR: u8 = 9;
const SET: u8 = 10;
const MAP: u8 = 11;
const HANDLE: u8 = 12;
const NAMESPACE: u8 = 13;
const VAR: u8 = 14;
const F64: u8 = 15;
const ATOM: u8 = 16;
const ARRAY: u8 = 17;
const OBJECT: u8 = 18;
const CHARACTER: u8 = 19;
const BIG_INTEGER: u8 = 20;
const DECIMAL: u8 = 21;
const REGEX: u8 = 22;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_frame_bytes: usize,
    pub max_nesting_depth: usize,
    pub max_collection_items: usize,
    pub max_text_bytes: usize,
    pub max_byte_span: usize,
    pub allow_native_handles: bool,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_frame_bytes: 64 * 1024 * 1024,
            max_nesting_depth: 256,
            max_collection_items: 100_000,
            max_text_bytes: 16 * 1024 * 1024,
            max_byte_span: 64 * 1024 * 1024,
            allow_native_handles: false,
        }
    }
}

impl Limits {
    fn validate(self) -> Result<Self, Error> {
        if self.max_frame_bytes < MAGIC.len() + 1 {
            return Err(Error::InvalidLimits("max_frame_bytes is too small"));
        }
        if self.max_collection_items == 0 {
            return Err(Error::InvalidLimits(
                "max_collection_items must be positive",
            ));
        }
        if self.max_text_bytes == 0 {
            return Err(Error::InvalidLimits("max_text_bytes must be positive"));
        }
        if self.max_byte_span == 0 {
            return Err(Error::InvalidLimits("max_byte_span must be positive"));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Nil,
    Boolean,
    Integer,
    Float,
    Character,
    String,
    Bytes,
    Keyword,
    Symbol,
    BigInteger,
    Decimal,
    Regex,
    List,
    Vector,
    Set,
    Map,
    Namespace,
    Var,
    Atom,
    Array,
    Object,
    Handle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    InvalidLimits(&'static str),
    FrameTooLarge {
        limit: usize,
        actual: usize,
    },
    InvalidHeader,
    UnexpectedEof {
        offset: usize,
        needed: usize,
    },
    TrailingBytes {
        offset: usize,
    },
    UnknownTag {
        offset: usize,
        tag: u8,
    },
    NestingTooDeep {
        limit: usize,
        offset: usize,
    },
    CollectionTooLarge {
        limit: usize,
        actual: usize,
        offset: usize,
    },
    TextTooLarge {
        limit: usize,
        actual: usize,
        offset: usize,
    },
    ByteSpanTooLarge {
        limit: usize,
        actual: usize,
        offset: usize,
    },
    InvalidUtf8 {
        offset: usize,
    },
    InvalidCharacter {
        offset: usize,
        value: u32,
    },
    NativeHandleForbidden {
        offset: usize,
    },
    NonCanonicalOrder {
        offset: usize,
    },
    DuplicateObjectKey {
        offset: usize,
    },
    WrongKind {
        expected: &'static str,
        actual: Kind,
    },
    IndexOutOfBounds {
        index: usize,
        len: usize,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLimits(message) => write!(formatter, "invalid HTA limits: {message}"),
            Self::FrameTooLarge { limit, actual } => {
                write!(formatter, "HTA frame is {actual} bytes with limit {limit}")
            }
            Self::InvalidHeader => formatter.write_str("invalid HTA1 header"),
            Self::UnexpectedEof { offset, needed } => {
                write!(
                    formatter,
                    "truncated HTA value at byte {offset}; need {needed} bytes"
                )
            }
            Self::TrailingBytes { offset } => write!(formatter, "trailing HTA bytes at {offset}"),
            Self::UnknownTag { offset, tag } => {
                write!(formatter, "unknown HTA tag {tag} at {offset}")
            }
            Self::NestingTooDeep { limit, offset } => {
                write!(formatter, "HTA nesting exceeds {limit} at {offset}")
            }
            Self::CollectionTooLarge {
                limit,
                actual,
                offset,
            } => write!(
                formatter,
                "HTA collection at {offset} has {actual} items with limit {limit}"
            ),
            Self::TextTooLarge {
                limit,
                actual,
                offset,
            } => write!(
                formatter,
                "HTA text at {offset} is {actual} bytes with limit {limit}"
            ),
            Self::ByteSpanTooLarge {
                limit,
                actual,
                offset,
            } => write!(
                formatter,
                "HTA byte span at {offset} is {actual} bytes with limit {limit}"
            ),
            Self::InvalidUtf8 { offset } => write!(formatter, "invalid UTF-8 at {offset}"),
            Self::InvalidCharacter { offset, value } => {
                write!(formatter, "invalid Unicode scalar {value:#x} at {offset}")
            }
            Self::NativeHandleForbidden { offset } => {
                write!(formatter, "native HTA handle is forbidden at {offset}")
            }
            Self::NonCanonicalOrder { offset } => {
                write!(
                    formatter,
                    "non-canonical or duplicate HTA entry at {offset}"
                )
            }
            Self::DuplicateObjectKey { offset } => {
                write!(formatter, "duplicate HTA object key at {offset}")
            }
            Self::WrongKind { expected, actual } => {
                write!(formatter, "expected HTA {expected}, got {actual:?}")
            }
            Self::IndexOutOfBounds { index, len } => {
                write!(
                    formatter,
                    "HTA index {index} is outside collection length {len}"
                )
            }
        }
    }
}

impl std::error::Error for Error {}

#[derive(Debug)]
enum Payload {
    Empty,
    Boolean(bool),
    Integer(i64),
    Float(u64),
    Character(char),
    Bytes(Range<usize>),
    Sequence(Range<usize>),
    Map(Range<usize>),
    Handle {
        provider: Range<usize>,
        type_name: Range<usize>,
        handle: u64,
    },
}

#[derive(Debug)]
struct NodeData {
    kind: Kind,
    span: Range<usize>,
    payload: Payload,
}

#[derive(Debug)]
pub struct Document<'a> {
    frame: &'a [u8],
    nodes: Vec<NodeData>,
    children: Vec<usize>,
    pairs: Vec<(usize, usize)>,
    root: usize,
}

impl<'a> Document<'a> {
    pub fn parse(frame: &'a [u8]) -> Result<Self, Error> {
        Self::parse_with(frame, Limits::default())
    }

    pub fn parse_with(frame: &'a [u8], limits: Limits) -> Result<Self, Error> {
        let limits = limits.validate()?;
        if frame.len() > limits.max_frame_bytes {
            return Err(Error::FrameTooLarge {
                limit: limits.max_frame_bytes,
                actual: frame.len(),
            });
        }
        if !frame.starts_with(MAGIC) {
            return Err(Error::InvalidHeader);
        }
        let mut parser = Parser {
            frame,
            cursor: MAGIC.len(),
            limits,
            nodes: Vec::new(),
            children: Vec::new(),
            pairs: Vec::new(),
        };
        let root = parser.value(0)?;
        if parser.cursor != frame.len() {
            return Err(Error::TrailingBytes {
                offset: parser.cursor,
            });
        }
        Ok(Self {
            frame,
            nodes: parser.nodes,
            children: parser.children,
            pairs: parser.pairs,
            root,
        })
    }

    pub fn root(&self) -> Node<'_, 'a> {
        self.node(self.root)
    }

    fn node(&self, index: usize) -> Node<'_, 'a> {
        Node {
            document: self,
            index,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Node<'d, 'a> {
    document: &'d Document<'a>,
    index: usize,
}

impl fmt::Debug for Node<'_, '_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Node")
            .field("kind", &self.kind())
            .field("span", &self.data().span)
            .finish()
    }
}

impl<'d, 'a> Node<'d, 'a> {
    fn data(self) -> &'d NodeData {
        &self.document.nodes[self.index]
    }

    pub fn kind(self) -> Kind {
        self.data().kind
    }

    pub fn encoded(self) -> &'a [u8] {
        &self.document.frame[self.data().span.clone()]
    }

    pub fn standalone_frame(self) -> Vec<u8> {
        let mut output = Vec::with_capacity(MAGIC.len() + self.encoded().len());
        output.extend_from_slice(MAGIC);
        output.extend_from_slice(self.encoded());
        output
    }

    pub fn as_bool(self) -> Result<bool, Error> {
        match &self.data().payload {
            Payload::Boolean(value) => Ok(*value),
            _ => Err(self.wrong("boolean")),
        }
    }

    pub fn as_i64(self) -> Result<i64, Error> {
        match &self.data().payload {
            Payload::Integer(value) => Ok(*value),
            _ => Err(self.wrong("integer")),
        }
    }

    pub fn as_f64(self) -> Result<f64, Error> {
        match &self.data().payload {
            Payload::Float(bits) => Ok(f64::from_bits(*bits)),
            _ => Err(self.wrong("float")),
        }
    }

    pub fn as_character(self) -> Result<char, Error> {
        match &self.data().payload {
            Payload::Character(value) => Ok(*value),
            _ => Err(self.wrong("character")),
        }
    }

    pub fn as_bytes(self) -> Result<&'a [u8], Error> {
        match &self.data().payload {
            Payload::Bytes(range) => Ok(&self.document.frame[range.clone()]),
            _ => Err(self.wrong("byte or text value")),
        }
    }

    pub fn as_text(self) -> Result<&'a str, Error> {
        if !matches!(
            self.kind(),
            Kind::String
                | Kind::Keyword
                | Kind::Symbol
                | Kind::Namespace
                | Kind::BigInteger
                | Kind::Decimal
                | Kind::Regex
        ) {
            return Err(self.wrong("text value"));
        }
        str::from_utf8(self.as_bytes()?).map_err(|_| Error::InvalidUtf8 {
            offset: self.data().span.start,
        })
    }

    pub fn len(self) -> Result<usize, Error> {
        match &self.data().payload {
            Payload::Sequence(range) | Payload::Map(range) => Ok(range.len()),
            _ => Err(self.wrong("collection")),
        }
    }

    pub fn get(self, index: usize) -> Result<Node<'d, 'a>, Error> {
        let Payload::Sequence(range) = &self.data().payload else {
            return Err(self.wrong("sequence"));
        };
        let child = self
            .document
            .children
            .get(range.start + index)
            .filter(|_| index < range.len())
            .copied()
            .ok_or(Error::IndexOutOfBounds {
                index,
                len: range.len(),
            })?;
        Ok(self.document.node(child))
    }

    pub fn pair(self, index: usize) -> Result<(Node<'d, 'a>, Node<'d, 'a>), Error> {
        let Payload::Map(range) = &self.data().payload else {
            return Err(self.wrong("map or object"));
        };
        let (key, value) = self
            .document
            .pairs
            .get(range.start + index)
            .filter(|_| index < range.len())
            .copied()
            .ok_or(Error::IndexOutOfBounds {
                index,
                len: range.len(),
            })?;
        Ok((self.document.node(key), self.document.node(value)))
    }

    pub fn map_get(self, name: &str) -> Result<Option<Node<'d, 'a>>, Error> {
        let len = self.len()?;
        for index in 0..len {
            let (key, value) = self.pair(index)?;
            if matches!(key.kind(), Kind::String | Kind::Keyword) && key.as_text()? == name {
                return Ok(Some(value));
            }
        }
        Ok(None)
    }

    pub fn require(self, name: &str) -> Result<Node<'d, 'a>, Error> {
        self.map_get(name)?.ok_or(Error::WrongKind {
            expected: "required map field",
            actual: self.kind(),
        })
    }

    pub fn handle(self) -> Result<(&'a str, &'a str, u64), Error> {
        let Payload::Handle {
            provider,
            type_name,
            handle,
        } = &self.data().payload
        else {
            return Err(self.wrong("native handle"));
        };
        let provider = str::from_utf8(&self.document.frame[provider.clone()]).map_err(|_| {
            Error::InvalidUtf8 {
                offset: provider.start,
            }
        })?;
        let type_name = str::from_utf8(&self.document.frame[type_name.clone()]).map_err(|_| {
            Error::InvalidUtf8 {
                offset: type_name.start,
            }
        })?;
        Ok((provider, type_name, *handle))
    }

    fn wrong(self, expected: &'static str) -> Error {
        Error::WrongKind {
            expected,
            actual: self.kind(),
        }
    }
}

struct Parser<'a> {
    frame: &'a [u8],
    cursor: usize,
    limits: Limits,
    nodes: Vec<NodeData>,
    children: Vec<usize>,
    pairs: Vec<(usize, usize)>,
}

impl Parser<'_> {
    fn value(&mut self, depth: usize) -> Result<usize, Error> {
        if depth > self.limits.max_nesting_depth {
            return Err(Error::NestingTooDeep {
                limit: self.limits.max_nesting_depth,
                offset: self.cursor,
            });
        }
        let start = self.cursor;
        let tag = self.byte()?;
        let (kind, payload) = match tag {
            NIL => (Kind::Nil, Payload::Empty),
            FALSE => (Kind::Boolean, Payload::Boolean(false)),
            TRUE => (Kind::Boolean, Payload::Boolean(true)),
            I64 => {
                let bytes = self.take(8)?;
                (
                    Kind::Integer,
                    Payload::Integer(i64::from_be_bytes(
                        bytes.try_into().expect("8 bytes"),
                    )),
                )
            }
            F64 => {
                let bytes = self.take(8)?;
                (
                    Kind::Float,
                    Payload::Float(u64::from_be_bytes(
                        bytes.try_into().expect("8 bytes"),
                    )),
                )
            }
            CHARACTER => {
                let offset = self.cursor;
                let bytes = self.take(4)?;
                let value = u32::from_be_bytes(bytes.try_into().expect("4 bytes"));
                let character =
                    char::from_u32(value).ok_or(Error::InvalidCharacter { offset, value })?;
                (Kind::Character, Payload::Character(character))
            }
            STRING => (Kind::String, Payload::Bytes(self.text_span()?)),
            BYTES => (Kind::Bytes, Payload::Bytes(self.byte_span()?)),
            KEYWORD => (Kind::Keyword, Payload::Bytes(self.text_span()?)),
            SYMBOL => (Kind::Symbol, Payload::Bytes(self.text_span()?)),
            BIG_INTEGER => (Kind::BigInteger, Payload::Bytes(self.text_span()?)),
            DECIMAL => (Kind::Decimal, Payload::Bytes(self.text_span()?)),
            REGEX => (Kind::Regex, Payload::Bytes(self.text_span()?)),
            NAMESPACE => (Kind::Namespace, Payload::Bytes(self.text_span()?)),
            LIST => (Kind::List, Payload::Sequence(self.sequence(depth)?)),
            VECTOR => (Kind::Vector, Payload::Sequence(self.sequence(depth)?)),
            ARRAY => (Kind::Array, Payload::Sequence(self.sequence(depth)?)),
            SET => (Kind::Set, Payload::Sequence(self.set(depth)?)),
            MAP => (Kind::Map, Payload::Map(self.map(depth, true)?)),
            OBJECT => (Kind::Object, Payload::Map(self.object(depth)?)),
            VAR => {
                let symbol = self.value(depth + 1)?;
                let value = self.value(depth + 1)?;
                let begin = self.children.len();
                self.children.extend([symbol, value]);
                (
                    Kind::Var,
                    Payload::Sequence(begin..self.children.len()),
                )
            }
            ATOM => {
                let value = self.value(depth + 1)?;
                let begin = self.children.len();
                self.children.push(value);
                (
                    Kind::Atom,
                    Payload::Sequence(begin..self.children.len()),
                )
            }
            HANDLE => {
                if !self.limits.allow_native_handles {
                    return Err(Error::NativeHandleForbidden { offset: start });
                }
                let provider = self.text_span()?;
                let type_name = self.text_span()?;
                let bytes = self.take(8)?;
                let handle = u64::from_be_bytes(bytes.try_into().expect("8 bytes"));
                (
                    Kind::Handle,
                    Payload::Handle {
                        provider,
                        type_name,
                        handle,
                    },
                )
            }
            _ => return Err(Error::UnknownTag { offset: start, tag }),
        };
        let index = self.nodes.len();
        self.nodes.push(NodeData {
            kind,
            span: start..self.cursor,
            payload,
        });
        Ok(index)
    }

    fn sequence(&mut self, depth: usize) -> Result<Range<usize>, Error> {
        let count = self.count()?;
        let mut parsed = Vec::with_capacity(count);
        for _ in 0..count {
            parsed.push(self.value(depth + 1)?);
        }
        let begin = self.children.len();
        self.children.extend(parsed);
        Ok(begin..self.children.len())
    }

    fn set(&mut self, depth: usize) -> Result<Range<usize>, Error> {
        let count = self.count()?;
        let mut parsed = Vec::with_capacity(count);
        let mut previous: Option<Range<usize>> = None;
        for _ in 0..count {
            let child = self.value(depth + 1)?;
            let span = self.nodes[child].span.clone();
            if previous
                .as_ref()
                .is_some_and(|prior| self.frame[prior.clone()] >= self.frame[span.clone()])
            {
                return Err(Error::NonCanonicalOrder { offset: span.start });
            }
            previous = Some(span);
            parsed.push(child);
        }
        let begin = self.children.len();
        self.children.extend(parsed);
        Ok(begin..self.children.len())
    }

    fn map(&mut self, depth: usize, canonical: bool) -> Result<Range<usize>, Error> {
        let count = self.count()?;
        let mut parsed = Vec::with_capacity(count);
        let mut previous: Option<Range<usize>> = None;
        for _ in 0..count {
            let key = self.value(depth + 1)?;
            let key_span = self.nodes[key].span.clone();
            if canonical
                && previous.as_ref().is_some_and(|prior| {
                    self.frame[prior.clone()] >= self.frame[key_span.clone()]
                })
            {
                return Err(Error::NonCanonicalOrder {
                    offset: key_span.start,
                });
            }
            let value = self.value(depth + 1)?;
            previous = Some(key_span);
            parsed.push((key, value));
        }
        let begin = self.pairs.len();
        self.pairs.extend(parsed);
        Ok(begin..self.pairs.len())
    }

    fn object(&mut self, depth: usize) -> Result<Range<usize>, Error> {
        let range = self.map(depth, false)?;
        for index in range.clone() {
            let key = self.pairs[index].0;
            if self.nodes[key].kind != Kind::String {
                return Err(Error::WrongKind {
                    expected: "object string key",
                    actual: self.nodes[key].kind,
                });
            }
            let key_bytes = match &self.nodes[key].payload {
                Payload::Bytes(bytes) => &self.frame[bytes.clone()],
                _ => unreachable!("string node has byte payload"),
            };
            for prior in range.start..index {
                let prior_key = self.pairs[prior].0;
                let prior_bytes = match &self.nodes[prior_key].payload {
                    Payload::Bytes(bytes) => &self.frame[bytes.clone()],
                    _ => unreachable!("string node has byte payload"),
                };
                if prior_bytes == key_bytes {
                    return Err(Error::DuplicateObjectKey {
                        offset: self.nodes[key].span.start,
                    });
                }
            }
        }
        Ok(range)
    }

    fn text_span(&mut self) -> Result<Range<usize>, Error> {
        let offset = self.cursor;
        let span = self.sized_span(self.limits.max_text_bytes, true)?;
        str::from_utf8(&self.frame[span.clone()]).map_err(|_| Error::InvalidUtf8 { offset })?;
        Ok(span)
    }

    fn byte_span(&mut self) -> Result<Range<usize>, Error> {
        self.sized_span(self.limits.max_byte_span, false)
    }

    fn sized_span(&mut self, limit: usize, text: bool) -> Result<Range<usize>, Error> {
        let offset = self.cursor;
        let length = self.u32()? as usize;
        if length > limit {
            return Err(if text {
                Error::TextTooLarge {
                    limit,
                    actual: length,
                    offset,
                }
            } else {
                Error::ByteSpanTooLarge {
                    limit,
                    actual: length,
                    offset,
                }
            });
        }
        let start = self.cursor;
        self.take(length)?;
        Ok(start..self.cursor)
    }

    fn count(&mut self) -> Result<usize, Error> {
        let offset = self.cursor;
        let count = self.u32()? as usize;
        if count > self.limits.max_collection_items {
            return Err(Error::CollectionTooLarge {
                limit: self.limits.max_collection_items,
                actual: count,
                offset,
            });
        }
        Ok(count)
    }

    fn byte(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, Error> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes(bytes.try_into().expect("4 bytes")))
    }

    fn take(&mut self, length: usize) -> Result<&[u8], Error> {
        if length > self.frame.len().saturating_sub(self.cursor) {
            return Err(Error::UnexpectedEof {
                offset: self.cursor,
                needed: length,
            });
        }
        let start = self.cursor;
        self.cursor += length;
        Ok(&self.frame[start..self.cursor])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(value: &[u8]) -> Vec<u8> {
        let mut output = MAGIC.to_vec();
        output.extend_from_slice(value);
        output
    }

    fn text(tag: u8, value: &str) -> Vec<u8> {
        let mut output = vec![tag];
        output.extend_from_slice(&(value.len() as u32).to_be_bytes());
        output.extend_from_slice(value.as_bytes());
        output
    }

    fn integer(value: i64) -> Vec<u8> {
        let mut output = vec![I64];
        output.extend_from_slice(&value.to_be_bytes());
        output
    }

    fn vector(values: &[Vec<u8>]) -> Vec<u8> {
        let mut output = vec![VECTOR];
        output.extend_from_slice(&(values.len() as u32).to_be_bytes());
        for value in values {
            output.extend_from_slice(value);
        }
        output
    }

    fn map(mut pairs: Vec<(Vec<u8>, Vec<u8>)>) -> Vec<u8> {
        pairs.sort_by(|left, right| left.0.cmp(&right.0));
        let mut output = vec![MAP];
        output.extend_from_slice(&(pairs.len() as u32).to_be_bytes());
        for (key, value) in pairs {
            output.extend_from_slice(&key);
            output.extend_from_slice(&value);
        }
        output
    }

    #[test]
    fn preserves_nested_value_and_receipt_spans_exactly() {
        let opaque_value = map(vec![
            (text(KEYWORD, "alpha"), integer(7)),
            (
                text(KEYWORD, "nested"),
                vector(&[text(STRING, "x"), integer(9)]),
            ),
        ]);
        let opaque_receipt = vector(&[text(KEYWORD, "receipt"), integer(3)]);
        let request = map(vec![
            (
                text(KEYWORD, "protocol"),
                text(STRING, "hara.store-request/1"),
            ),
            (
                text(KEYWORD, "operation"),
                text(STRING, "compare-and-swap"),
            ),
            (text(KEYWORD, "value"), opaque_value.clone()),
            (text(KEYWORD, "receipt"), opaque_receipt.clone()),
        ]);
        let arguments = frame(&vector(&[request]));
        let document = Document::parse(&arguments).unwrap();
        let request = document.root().get(0).unwrap();
        let value = request.map_get("value").unwrap().unwrap();
        let receipt = request.map_get("receipt").unwrap().unwrap();
        assert_eq!(value.encoded(), opaque_value);
        assert_eq!(receipt.encoded(), opaque_receipt);
        assert_eq!(
            Document::parse(&value.standalone_frame())
                .unwrap()
                .root()
                .encoded(),
            opaque_value
        );
    }

    #[test]
    fn exposes_closed_accessors() {
        let request = map(vec![
            (text(KEYWORD, "operation"), text(STRING, "load")),
            (text(KEYWORD, "revision"), integer(4)),
        ]);
        let document = Document::parse(&frame(&vector(&[request]))).unwrap();
        let request = document.root().get(0).unwrap();
        assert_eq!(request.len().unwrap(), 2);
        assert_eq!(
            request
                .map_get("operation")
                .unwrap()
                .unwrap()
                .as_text()
                .unwrap(),
            "load"
        );
        assert_eq!(
            request
                .map_get("revision")
                .unwrap()
                .unwrap()
                .as_i64()
                .unwrap(),
            4
        );
        assert!(request.map_get("missing").unwrap().is_none());
    }

    #[test]
    fn rejects_truncation_and_trailing_bytes() {
        let valid = frame(&integer(4));
        assert!(matches!(
            Document::parse(&valid[..valid.len() - 1]),
            Err(Error::UnexpectedEof { .. })
        ));
        let mut trailing = valid;
        trailing.push(NIL);
        assert!(matches!(
            Document::parse(&trailing),
            Err(Error::TrailingBytes { .. })
        ));
    }

    #[test]
    fn rejects_noncanonical_and_duplicate_map_keys() {
        let key_a = text(KEYWORD, "a");
        let key_b = text(KEYWORD, "b");
        let mut reversed = vec![MAP];
        reversed.extend_from_slice(&2_u32.to_be_bytes());
        reversed.extend_from_slice(&key_b);
        reversed.push(NIL);
        reversed.extend_from_slice(&key_a);
        reversed.push(NIL);
        assert!(matches!(
            Document::parse(&frame(&reversed)),
            Err(Error::NonCanonicalOrder { .. })
        ));

        let mut duplicate = vec![MAP];
        duplicate.extend_from_slice(&2_u32.to_be_bytes());
        duplicate.extend_from_slice(&key_a);
        duplicate.push(NIL);
        duplicate.extend_from_slice(&key_a);
        duplicate.push(TRUE);
        assert!(matches!(
            Document::parse(&frame(&duplicate)),
            Err(Error::NonCanonicalOrder { .. })
        ));
    }

    #[test]
    fn rejects_noncanonical_sets() {
        let a = text(STRING, "a");
        let b = text(STRING, "b");
        let mut set = vec![SET];
        set.extend_from_slice(&2_u32.to_be_bytes());
        set.extend_from_slice(&b);
        set.extend_from_slice(&a);
        assert!(matches!(
            Document::parse(&frame(&set)),
            Err(Error::NonCanonicalOrder { .. })
        ));
    }

    #[test]
    fn rejects_invalid_utf8_and_forbidden_handles() {
        let malformed = frame(&[STRING, 0, 0, 0, 1, 0xff]);
        assert!(matches!(
            Document::parse(&malformed),
            Err(Error::InvalidUtf8 { .. })
        ));

        let mut handle = vec![HANDLE];
        handle.extend_from_slice(&1_u32.to_be_bytes());
        handle.push(b'p');
        handle.extend_from_slice(&1_u32.to_be_bytes());
        handle.push(b't');
        handle.extend_from_slice(&1_u64.to_be_bytes());
        assert!(matches!(
            Document::parse(&frame(&handle)),
            Err(Error::NativeHandleForbidden { .. })
        ));
        let limits = Limits {
            allow_native_handles: true,
            ..Limits::default()
        };
        assert_eq!(
            Document::parse_with(&frame(&handle), limits)
                .unwrap()
                .root()
                .handle()
                .unwrap(),
            ("p", "t", 1)
        );
    }

    #[test]
    fn enforces_configured_limits() {
        let value = frame(&vector(&[integer(1), integer(2)]));
        let limits = Limits {
            max_collection_items: 1,
            ..Limits::default()
        };
        assert!(matches!(
            Document::parse_with(&value, limits),
            Err(Error::CollectionTooLarge { .. })
        ));

        let value = frame(&text(STRING, "ab"));
        let limits = Limits {
            max_text_bytes: 1,
            ..Limits::default()
        };
        assert!(matches!(
            Document::parse_with(&value, limits),
            Err(Error::TextTooLarge { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_object_keys() {
        let key = text(STRING, "same");
        let mut object = vec![OBJECT];
        object.extend_from_slice(&2_u32.to_be_bytes());
        object.extend_from_slice(&key);
        object.push(NIL);
        object.extend_from_slice(&key);
        object.push(TRUE);
        assert!(matches!(
            Document::parse(&frame(&object)),
            Err(Error::DuplicateObjectKey { .. })
        ));
    }
}
